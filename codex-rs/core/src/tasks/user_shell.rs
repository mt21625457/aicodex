use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use codex_async_utils::CancelErr;
use codex_async_utils::OrCancelExt;
use codex_network_proxy::PROXY_ACTIVE_ENV_KEY;
use codex_utils_absolute_path::AbsolutePathBuf;
use tokio_util::sync::CancellationToken;
use tracing::error;
use tracing::warn;
use uuid::Uuid;

use crate::exec::ExecCapturePolicy;
use crate::exec::StdoutStream;
use crate::exec::execute_exec_request;
use crate::exec_env::create_env;
use crate::exec_env::inject_apply_patch_env;
use crate::exec_env::inject_session_id_env;
use crate::sandboxing::ExecRequest;
use crate::session::TurnInput;
use crate::session::turn_context::TurnContext;
use crate::shell::Shell;
use crate::state::TaskKind;
use crate::tools::format_exec_output_str;
use crate::tools::runtimes::RuntimePathPrepends;
#[cfg(unix)]
use crate::tools::runtimes::apply_package_path_prepend;
use crate::tools::runtimes::maybe_wrap_shell_lc_with_snapshot;
use crate::tools::runtimes::strip_managed_proxy_env;
use crate::turn_timing::now_unix_timestamp_ms;
use crate::turn_timing::record_turn_ttfm_metric;
use crate::user_shell_command::user_shell_command_record_item;
use codex_protocol::error::CodexErrorDetails;
use codex_protocol::error::SandboxErr;
use codex_protocol::exec_output::ExecToolCallOutput;
use codex_protocol::exec_output::StreamOutput;
use codex_protocol::items::CommandExecutionItem;
use codex_protocol::items::CommandExecutionStatus;
use codex_protocol::items::TurnItem;
use codex_protocol::protocol::ErrorEvent;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ExecCommandSource;
use codex_protocol::protocol::HasLegacyEvent;
use codex_protocol::protocol::ItemCompletedEvent;
use codex_protocol::protocol::TurnStartedEvent;
use codex_rollout::EventPersistenceMode;
use codex_rollout::RolloutItem;
use codex_sandboxing::SandboxType;
use codex_shell_command::parse_command::parse_command;
use codex_thread_store::PersistContext;

use super::SessionTask;
use super::SessionTaskResult;
use crate::session::session::Session;
use codex_protocol::models::PermissionProfile;

const USER_SHELL_TIMEOUT_MS: u64 = 60 * 60 * 1000; // 1 hour

async fn emit_user_shell_item_completed(
    session: &Session,
    turn_context: &TurnContext,
    live_item: CommandExecutionItem,
    persisted_item: CommandExecutionItem,
) {
    let live_item = TurnItem::CommandExecution(live_item);
    record_turn_ttfm_metric(turn_context, &live_item).await;
    let completed_at_ms = now_unix_timestamp_ms();
    let item_id = live_item.id();
    let started_at_ms = turn_context
        .turn_timing_state
        .take_item_started(&item_id)
        .await
        .unwrap_or_else(|| {
            warn!(
                thread_id = %session.thread_id,
                turn_id = %turn_context.sub_id,
                item_id = %item_id,
                "user shell item completed without a recorded start timestamp"
            );
            completed_at_ms
        });
    session
        .send_event(
            turn_context,
            EventMsg::ItemCompleted(ItemCompletedEvent {
                thread_id: session.thread_id,
                turn_id: turn_context.sub_id.clone(),
                item: live_item,
                started_at_ms: Some(started_at_ms),
                completed_at_ms,
            }),
        )
        .await;

    // Legacy histories reconstruct command items from ExecCommandEnd rather than ItemCompleted.
    // Persist that projection separately so the live item can retain diagnostic text while
    // history keeps the command's clean output. Appending it after the live event also makes it
    // the final value when paginated histories replay both representations.
    let persisted_event = ItemCompletedEvent {
        thread_id: session.thread_id,
        turn_id: turn_context.sub_id.clone(),
        item: TurnItem::CommandExecution(persisted_item),
        started_at_ms: Some(started_at_ms),
        completed_at_ms,
    };
    let rollout_items = persisted_event
        .as_legacy_events(/*show_raw_agent_reasoning*/ false)
        .into_iter()
        .map(RolloutItem::EventMsg)
        .collect::<Vec<_>>();
    session
        .persist_rollout_items_with_mode(&rollout_items, EventPersistenceMode::Extended)
        .await;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum UserShellCommandMode {
    /// Executes as an independent turn lifecycle (emits TurnStarted/TurnComplete
    /// via task lifecycle plumbing).
    StandaloneTurn,
    /// Executes while another turn is already active. This mode must not emit a
    /// second TurnStarted/TurnComplete pair for the same active turn.
    ActiveTurnAuxiliary,
}

#[derive(Clone)]
pub(crate) struct UserShellCommandTask {
    command: String,
    timeout_ms: Option<u64>,
}

impl UserShellCommandTask {
    pub(crate) fn new(command: String, timeout_ms: Option<u64>) -> Self {
        Self {
            command,
            timeout_ms,
        }
    }
}

impl SessionTask for UserShellCommandTask {
    fn kind(&self) -> TaskKind {
        TaskKind::Regular
    }

    fn span_name(&self) -> &'static str {
        "session_task.user_shell"
    }

    async fn run(
        self: Arc<Self>,
        session: Arc<Session>,
        turn_context: Arc<TurnContext>,
        _input: Vec<TurnInput>,
        cancellation_token: CancellationToken,
    ) -> SessionTaskResult {
        execute_user_shell_command(
            session,
            turn_context,
            self.command.clone(),
            self.timeout_ms,
            cancellation_token,
            UserShellCommandMode::StandaloneTurn,
        )
        .await;
        Ok(None)
    }
}

pub(crate) async fn execute_user_shell_command(
    session: Arc<Session>,
    turn_context: Arc<TurnContext>,
    command: String,
    timeout_ms: Option<u64>,
    cancellation_token: CancellationToken,
    mode: UserShellCommandMode,
) {
    session
        .services
        .session_telemetry
        .counter("codex.task.user_shell", /*inc*/ 1, &[]);

    if mode == UserShellCommandMode::StandaloneTurn {
        // Auxiliary mode runs within an existing active turn. That turn already
        // emitted TurnStarted, so emitting another TurnStarted here would create
        // duplicate turn lifecycle events and confuse clients.
        // TODO(ccunningham): After TurnStarted, emit model-visible turn context diffs for
        // standalone lifecycle tasks (for example /shell, and review once it emits TurnStarted).
        // `/compact` is an intentional exception because compaction requests should not include
        // freshly reinjected context before the summary/replacement history is applied.
        let event = EventMsg::TurnStarted(TurnStartedEvent {
            turn_id: turn_context.sub_id.clone(),
            trace_id: turn_context.trace_id.clone(),
            started_at: turn_context.turn_timing_state.started_at_unix_secs().await,
            model_context_window: turn_context.model_context_window(),
            collaboration_mode_kind: turn_context.mode(),
        });
        session.send_event(turn_context.as_ref(), event).await;
    }

    let Some((turn_environment, environment_shell)) = turn_context
        .environments
        .local()
        .and_then(|environment| environment.shell.as_ref().map(|shell| (environment, shell)))
    else {
        send_user_shell_error(
            &session,
            turn_context.as_ref(),
            "shell is unavailable in this session",
        )
        .await;
        return;
    };

    // Execute the user's script under the environment's shell; this
    // allows commands that use shell features (pipes, &&, redirects, etc.).
    // We do not source rc files or otherwise reformat the script.
    let use_login_shell = true;
    let display_command = environment_shell.derive_exec_args(&command, use_login_shell);
    // TODO(anp): Migrate user-shell events and execution plumbing to PathUri so this local-only
    // feature does not need to project the selected environment cwd onto the Codex host.
    let Ok(cwd) = turn_environment.cwd().to_abs_path() else {
        send_user_shell_error(
            &session,
            turn_context.as_ref(),
            "shell working directory is not native to the Codex host",
        )
        .await;
        return;
    };
    let shell_snapshot_location = turn_environment.shell_snapshot(&cwd);
    let shell_environment_policy = turn_environment.shell_environment_policy();
    let mut exec_env_map = create_env(shell_environment_policy, Some(session.thread_id));
    inject_session_id_env(&mut exec_env_map, session.session_id());
    inject_apply_patch_env(&mut exec_env_map, &turn_context.config.features);
    if exec_env_map.contains_key(PROXY_ACTIVE_ENV_KEY) {
        strip_managed_proxy_env(&mut exec_env_map);
    }
    let exec_command = prepare_user_shell_exec_command(
        &display_command,
        environment_shell,
        shell_snapshot_location.as_ref(),
        &shell_environment_policy.r#set,
        &mut exec_env_map,
    );

    let call_id = Uuid::new_v4().to_string();
    let raw_command = command;

    let parsed_cmd = parse_command(&display_command);
    session
        .emit_turn_item_started(
            turn_context.as_ref(),
            &TurnItem::CommandExecution(CommandExecutionItem {
                id: call_id.clone(),
                plugin_id: None,
                script_path: None,
                process_id: None,
                command: display_command.clone(),
                cwd: cwd.clone().into(),
                parsed_cmd: parsed_cmd.clone(),
                source: ExecCommandSource::UserShell,
                interaction_input: None,
                status: CommandExecutionStatus::InProgress,
                stdout: None,
                stderr: None,
                aggregated_output: None,
                exit_code: None,
                duration: None,
                formatted_output: None,
            }),
        )
        .await;

    let permission_profile = PermissionProfile::Disabled;
    let exec_env = ExecRequest {
        command: exec_command.clone(),
        cwd: cwd.clone().into(),
        env: exec_env_map,
        exec_server_env_config: None,
        exec_server_shell_snapshot: None,
        // `/shell` is the explicit full-access escape hatch, so it must not
        // inherit a managed proxy from the surrounding session or turn.
        network: None,
        network_environment_id: None,
        expiration: timeout_ms.unwrap_or(USER_SHELL_TIMEOUT_MS).into(),
        capture_policy: ExecCapturePolicy::ShellTool,
        sandbox: SandboxType::None,
        windows_sandbox_policy_cwd: cwd.clone().into(),
        windows_sandbox_workspace_roots: turn_context.effective_workspace_roots(),
        windows_sandbox_level: turn_context.windows_sandbox_level,
        windows_sandbox_private_desktop: turn_context
            .config
            .permissions
            .windows_sandbox_private_desktop,
        permission_profile,
        windows_sandbox_filesystem_overrides: None,
        arg0: None,
        exec_server_sandbox: None,
        exec_server_enforce_managed_network: false,
        exec_server_managed_network: None,
        exec_server_network_proxy: None,
    };

    let stdout_stream = Some(StdoutStream {
        sub_id: turn_context.sub_id.clone(),
        call_id: call_id.clone(),
        tx_event: session.get_tx_event(),
    });

    let exec_result = execute_exec_request(exec_env, stdout_stream, /*after_spawn*/ None)
        .or_cancel(&cancellation_token)
        .await;

    match exec_result {
        Err(CancelErr::Cancelled) => {
            let aborted_message = "command aborted by user".to_string();
            let exec_output = ExecToolCallOutput {
                exit_code: -1,
                stdout: StreamOutput::new(String::new()),
                stderr: StreamOutput::new(aborted_message.clone()),
                aggregated_output: StreamOutput::new(aborted_message.clone()),
                duration: Duration::ZERO,
                timed_out: false,
            };
            persist_user_shell_output(
                &session,
                turn_context.as_ref(),
                &raw_command,
                &exec_output,
                mode,
            )
            .await;
            let completed_item = CommandExecutionItem {
                id: call_id,
                plugin_id: None,
                script_path: None,
                process_id: None,
                command: display_command.clone(),
                cwd: cwd.clone().into(),
                parsed_cmd: parsed_cmd.clone(),
                source: ExecCommandSource::UserShell,
                interaction_input: None,
                status: CommandExecutionStatus::Failed,
                stdout: Some(String::new()),
                stderr: Some(aborted_message.clone()),
                aggregated_output: Some(aborted_message.clone()),
                exit_code: Some(-1),
                duration: Some(Duration::ZERO),
                formatted_output: Some(aborted_message),
            };
            emit_user_shell_item_completed(
                session.as_ref(),
                turn_context.as_ref(),
                completed_item.clone(),
                completed_item,
            )
            .await;
        }
        Ok(Ok(output)) => {
            let completed_item = CommandExecutionItem {
                id: call_id.clone(),
                plugin_id: None,
                script_path: None,
                process_id: None,
                command: display_command.clone(),
                cwd: cwd.clone().into(),
                parsed_cmd: parsed_cmd.clone(),
                source: ExecCommandSource::UserShell,
                interaction_input: None,
                status: if output.exit_code == 0 {
                    CommandExecutionStatus::Completed
                } else {
                    CommandExecutionStatus::Failed
                },
                stdout: Some(output.stdout.text.clone()),
                stderr: Some(output.stderr.text.clone()),
                aggregated_output: Some(output.aggregated_output.text.clone()),
                exit_code: Some(output.exit_code),
                duration: Some(output.duration),
                formatted_output: Some(format_exec_output_str(
                    &output,
                    turn_context.model_info().truncation_policy.into(),
                )),
            };
            emit_user_shell_item_completed(
                session.as_ref(),
                turn_context.as_ref(),
                completed_item.clone(),
                completed_item,
            )
            .await;

            persist_user_shell_output(&session, turn_context.as_ref(), &raw_command, &output, mode)
                .await;
        }
        Ok(Err(err)) => {
            error!("user shell command failed: {err:?}");
            let persisted_timeout_output = match err.details() {
                CodexErrorDetails::Sandbox(SandboxErr::Timeout { output }) => {
                    Some(output.as_ref().clone())
                }
                _ => None,
            };
            let message = format!("execution error: {err:?}");
            let exec_output = ExecToolCallOutput {
                exit_code: -1,
                stdout: StreamOutput::new(String::new()),
                stderr: StreamOutput::new(message.clone()),
                aggregated_output: StreamOutput::new(message.clone()),
                duration: Duration::ZERO,
                timed_out: false,
            };
            let live_item = CommandExecutionItem {
                id: call_id,
                plugin_id: None,
                script_path: None,
                process_id: None,
                command: display_command,
                cwd: cwd.into(),
                parsed_cmd,
                source: ExecCommandSource::UserShell,
                interaction_input: None,
                status: CommandExecutionStatus::Failed,
                stdout: Some(exec_output.stdout.text.clone()),
                stderr: Some(exec_output.stderr.text.clone()),
                aggregated_output: Some(exec_output.aggregated_output.text.clone()),
                exit_code: Some(exec_output.exit_code),
                duration: Some(exec_output.duration),
                formatted_output: Some(format_exec_output_str(
                    &exec_output,
                    turn_context.model_info().truncation_policy.into(),
                )),
            };
            let persisted_output = persisted_timeout_output.as_ref().unwrap_or(&exec_output);
            let persisted_item = CommandExecutionItem {
                stdout: Some(persisted_output.stdout.text.clone()),
                stderr: Some(persisted_output.stderr.text.clone()),
                aggregated_output: Some(persisted_output.aggregated_output.text.clone()),
                formatted_output: Some(format_exec_output_str(
                    persisted_output,
                    turn_context.model_info().truncation_policy.into(),
                )),
                ..live_item.clone()
            };
            emit_user_shell_item_completed(
                session.as_ref(),
                turn_context.as_ref(),
                live_item,
                persisted_item,
            )
            .await;
            persist_user_shell_output(
                &session,
                turn_context.as_ref(),
                &raw_command,
                persisted_output,
                mode,
            )
            .await;
        }
    }
}

async fn send_user_shell_error(session: &Session, turn_context: &TurnContext, message: &str) {
    session
        .send_event(
            turn_context,
            EventMsg::Error(ErrorEvent {
                misalignment: None,
                message: message.to_string(),
                codex_error_info: None,
            }),
        )
        .await;
}

fn prepare_user_shell_exec_command(
    display_command: &[String],
    shell: &Shell,
    shell_snapshot: Option<&AbsolutePathBuf>,
    shell_environment_set: &HashMap<String, String>,
    exec_env_map: &mut HashMap<String, String>,
) -> Vec<String> {
    #[cfg(unix)]
    {
        prepare_user_shell_exec_command_with_path_prepend(
            display_command,
            shell,
            shell_snapshot,
            shell_environment_set,
            exec_env_map,
            apply_package_path_prepend,
        )
    }

    #[cfg(not(unix))]
    {
        maybe_wrap_shell_lc_with_snapshot(
            display_command,
            shell,
            shell_snapshot,
            shell_environment_set,
            exec_env_map,
            // On non-Unix targets, arg0 has already prepended the package path
            // to the process PATH before create_env() builds exec_env_map.
            // RuntimePathPrepends is only needed for Unix shell snapshot replay.
            &RuntimePathPrepends::default(),
        )
    }
}

/// Prepares a user-shell command after adding runtime-owned PATH entries.
///
/// The callback mutates the live exec environment for commands that are not
/// wrapped with a shell snapshot and records only the runtime-owned entries so
/// snapshot wrapping can reapply them after restoring the user's snapshot PATH.
#[cfg(unix)]
fn prepare_user_shell_exec_command_with_path_prepend(
    display_command: &[String],
    shell: &Shell,
    shell_snapshot: Option<&AbsolutePathBuf>,
    shell_environment_set: &HashMap<String, String>,
    exec_env_map: &mut HashMap<String, String>,
    prepend_runtime_path: impl FnOnce(&mut HashMap<String, String>, &mut RuntimePathPrepends),
) -> Vec<String> {
    let explicit_env_overrides = shell_environment_set.clone();
    let mut runtime_path_prepends = RuntimePathPrepends::default();
    prepend_runtime_path(exec_env_map, &mut runtime_path_prepends);
    maybe_wrap_shell_lc_with_snapshot(
        display_command,
        shell,
        shell_snapshot,
        &explicit_env_overrides,
        exec_env_map,
        &runtime_path_prepends,
    )
}

async fn persist_user_shell_output(
    session: &Session,
    turn_context: &TurnContext,
    raw_command: &str,
    exec_output: &ExecToolCallOutput,
    mode: UserShellCommandMode,
) {
    let output_item = user_shell_command_record_item(raw_command, exec_output, turn_context);

    if mode == UserShellCommandMode::StandaloneTurn {
        session
            .record_conversation_items(turn_context, std::slice::from_ref(&output_item))
            .await;
        // Standalone shell turns can run before any regular user turn, so
        // explicitly materialize rollout persistence after recording output.
        session
            .ensure_rollout_materialized(PersistContext::Standard)
            .await;
        return;
    }

    session
        .inject_no_new_turn(vec![output_item], Some(turn_context))
        .await;
}

#[cfg(all(test, unix))]
#[path = "user_shell_tests.rs"]
mod tests;
