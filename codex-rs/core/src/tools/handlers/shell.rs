use codex_features::Feature;
use codex_protocol::models::ShellCommandToolCallParams;
use serde_json::Value as JsonValue;
use std::sync::Arc;
use tokio_util::sync::CancellationToken;

use crate::exec::ExecParams;
use crate::exec_policy::ExecApprovalRequest;
use crate::function_tool::FunctionCallError;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;
use crate::session::turn_context::TurnEnvironment;
use crate::shell::ShellType;
use crate::tools::context::FunctionToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::events::ToolEmitter;
use crate::tools::events::ToolEventCtx;
use crate::tools::handlers::apply_granted_turn_permissions;
use crate::tools::handlers::apply_patch::intercept_apply_patch;
use crate::tools::handlers::implicit_granted_permissions;
use crate::tools::handlers::normalize_and_validate_additional_permissions;
use crate::tools::handlers::parse_arguments;
use crate::tools::orchestrator::ToolOrchestrator;
use crate::tools::runtimes::shell::ShellRequest;
use crate::tools::runtimes::shell::ShellRuntime;
use crate::tools::runtimes::shell::ShellRuntimeBackend;
use crate::tools::sandboxing::ToolCtx;
use codex_core_plugins::strip_output_env;
use codex_protocol::exec_output::ExecToolCallOutput;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_protocol::protocol::ExecCommandSource;
use codex_shell_command::is_safe_command::is_known_safe_command;
use codex_tools::ToolName;
use codex_utils_path_uri::PathUri;

mod shell_command;
mod shell_file_changes;

pub(crate) use shell_command::ClaudeBashHandler;
pub use shell_command::ShellCommandHandler;
pub(crate) use shell_command::ShellCommandHandlerOptions;
use shell_file_changes::diff_shell_snapshots;
use shell_file_changes::snapshot_shell_files;

fn shell_command_payload_command(payload: &ToolPayload) -> Option<String> {
    let ToolPayload::Function { arguments } = payload else {
        return None;
    };

    parse_arguments::<ShellCommandToolCallParams>(arguments)
        .ok()
        .map(|params| params.command)
}

struct RunExecLikeArgs {
    tool_name: ToolName,
    exec_params: ExecParams,
    cancellation_token: CancellationToken,
    hook_command: String,
    shell_type: Option<ShellType>,
    additional_permissions: Option<AdditionalPermissionProfile>,
    prefix_rule: Option<Vec<String>>,
    session: Arc<crate::session::session::Session>,
    step_context: Arc<StepContext>,
    turn_environment: TurnEnvironment,
    tracker: crate::tools::context::SharedTurnDiffTracker,
    call_id: String,
    shell_runtime_backend: ShellRuntimeBackend,
}

async fn run_exec_like(args: RunExecLikeArgs) -> Result<FunctionToolOutput, FunctionCallError> {
    let RunExecLikeArgs {
        tool_name,
        exec_params,
        cancellation_token,
        hook_command,
        shell_type,
        additional_permissions,
        prefix_rule,
        session,
        step_context,
        turn_environment,
        tracker,
        call_id,
        shell_runtime_backend,
    } = args;
    let turn = Arc::clone(&step_context.turn);

    let fs = turn_environment.environment.get_filesystem();

    let mut explicit_env_overrides = turn_environment.shell_environment_policy().r#set.clone();
    let mut env = exec_params.env.clone();
    strip_output_env(&mut env);
    strip_output_env(&mut explicit_env_overrides);
    let exec_permission_approvals_enabled =
        session.features().enabled(Feature::ExecPermissionApprovals);
    let requested_additional_permissions = additional_permissions.clone();
    let effective_additional_permissions = apply_granted_turn_permissions(
        session.as_ref(),
        &turn_environment.selection.environment_id,
        exec_params.cwd.as_path(),
        exec_params.sandbox_permissions,
        additional_permissions,
    )
    .await;
    let additional_permissions_allowed = exec_permission_approvals_enabled
        || (session.features().enabled(Feature::RequestPermissionsTool)
            && effective_additional_permissions.permissions_preapproved);
    let normalized_additional_permissions = implicit_granted_permissions(
        exec_params.sandbox_permissions,
        requested_additional_permissions.as_ref(),
        &effective_additional_permissions,
    )
    .map_or_else(
        || {
            normalize_and_validate_additional_permissions(
                additional_permissions_allowed,
                turn.approval_policy(),
                effective_additional_permissions.sandbox_permissions,
                effective_additional_permissions.additional_permissions,
                effective_additional_permissions.permissions_preapproved,
                &exec_params.cwd,
            )
        },
        |permissions| Ok(Some(permissions)),
    )
    .map_err(FunctionCallError::RespondToModel)?;

    // Approval policy guard for explicit escalation in non-OnRequest modes.
    // Sticky turn permissions have already been approved, so they should
    // continue through the normal exec approval flow for the command.
    if effective_additional_permissions
        .sandbox_permissions
        .requests_sandbox_override()
        && !effective_additional_permissions.permissions_preapproved
        && !matches!(
            turn.approval_policy(),
            codex_protocol::protocol::AskForApproval::OnRequest
        )
    {
        let approval_policy = turn.approval_policy();
        return Err(FunctionCallError::RespondToModel(format!(
            "approval policy is {approval_policy:?}; reject command — you should not ask for escalated permissions if the approval policy is {approval_policy:?}"
        )));
    }

    // Intercept apply_patch if present.
    let apply_patch_cwd = PathUri::from_abs_path(&exec_params.cwd);
    if let Some(output) = intercept_apply_patch(
        &exec_params.command,
        &apply_patch_cwd,
        fs.as_ref(),
        turn_environment.clone(),
        session.clone(),
        Arc::clone(&step_context),
        Some(&tracker),
        &call_id,
        tool_name.name.as_str(),
    )
    .await?
    {
        return Ok(output);
    }

    let shell_file_change_sandbox = turn_environment.environment.is_remote().then(|| {
        turn.file_system_sandbox_context(/*additional_permissions*/ None, &turn_environment)
    });
    let shell_file_snapshot_before = if is_known_safe_command(&exec_params.command) {
        None
    } else {
        snapshot_shell_files(
            fs.as_ref(),
            &exec_params.cwd,
            shell_file_change_sandbox.as_ref(),
        )
        .await
    };

    let source = ExecCommandSource::Agent;
    let plugin_attribution =
        turn.plugin_attribution_for_command(&exec_params.command, &exec_params.cwd);
    let emitter = ToolEmitter::shell(
        exec_params.command.clone(),
        exec_params.cwd.clone(),
        source,
        plugin_attribution,
    );
    let event_ctx = ToolEventCtx::new(
        session.as_ref(),
        turn.as_ref(),
        &call_id,
        /*turn_diff_tracker*/ None,
    );
    emitter.begin(event_ctx).await;

    let exec_approval_requirement = session
        .services
        .exec_policy
        .create_exec_approval_requirement_for_command(ExecApprovalRequest {
            command: &exec_params.command,
            approval_policy: turn.approval_policy(),
            permission_profile: turn_environment.permission_profile().clone(),
            environment_policy: turn_environment.config().exec_policy.as_ref(),
            windows_sandbox_level: turn.windows_sandbox_level,
            sandbox_permissions: if effective_additional_permissions.permissions_preapproved {
                codex_protocol::models::SandboxPermissions::UseDefault
            } else {
                effective_additional_permissions.sandbox_permissions
            },
            prefix_rule,
            allow_prefix_rules: turn.allow_prefix_rules(),
        })
        .await;

    let req = ShellRequest {
        command: exec_params.command.clone(),
        turn_environment: turn_environment.clone(),
        shell_type,
        hook_command,
        cwd: exec_params.cwd.clone(),
        timeout_ms: exec_params.expiration.timeout_ms(),
        cancellation_token,
        env,
        explicit_env_overrides,
        network: exec_params.network.clone(),
        sandbox_permissions: effective_additional_permissions.sandbox_permissions,
        additional_permissions: normalized_additional_permissions,
        #[cfg(unix)]
        additional_permissions_preapproved: effective_additional_permissions
            .permissions_preapproved,
        justification: exec_params.justification.clone(),
        exec_approval_requirement,
    };
    let mut orchestrator = ToolOrchestrator::new();
    let mut runtime = ShellRuntime::for_shell_command(shell_runtime_backend);
    let tool_ctx = ToolCtx {
        session: session.clone(),
        step_context,
        call_id: call_id.clone(),
        tool_name,
    };
    let out = orchestrator
        .run(&mut runtime, &req, &tool_ctx, &turn, turn.approval_policy())
        .await
        .map(|result| result.output);
    let shell_file_changes = match &shell_file_snapshot_before {
        Some(before) => {
            let after = snapshot_shell_files(
                fs.as_ref(),
                &exec_params.cwd,
                shell_file_change_sandbox.as_ref(),
            )
            .await;
            after
                .as_ref()
                .map(|after| diff_shell_snapshots(before, after))
                .filter(|changes| !changes.is_empty())
        }
        None => None,
    };
    let event_ctx = ToolEventCtx::new(
        session.as_ref(),
        turn.as_ref(),
        &call_id,
        /*turn_diff_tracker*/ None,
    );
    let post_tool_use_response = out
        .as_ref()
        .ok()
        .map(|output| {
            crate::tools::format_exec_output_str(output, turn.model_info.truncation_policy.into())
        })
        .map(JsonValue::String);
    let content = emitter
        .finish(event_ctx, out, /*applied_patch_delta*/ None)
        .await;
    if let Some(changes) = shell_file_changes {
        emit_shell_file_changes(
            session.as_ref(),
            turn.as_ref(),
            &call_id,
            &turn_environment.selection.environment_id,
            changes,
        )
        .await;
    }
    let content = content?;
    Ok(FunctionToolOutput {
        body: vec![
            codex_protocol::models::FunctionCallOutputContentItem::InputText { text: content },
        ],
        success: Some(true),
        post_tool_use_response,
    })
}

async fn emit_shell_file_changes(
    session: &crate::session::session::Session,
    turn: &TurnContext,
    shell_call_id: &str,
    environment_id: &str,
    changes: std::collections::HashMap<std::path::PathBuf, codex_protocol::protocol::FileChange>,
) {
    let call_id = format!("{shell_call_id}-file-change");
    let emitter = ToolEmitter::apply_patch_for_environment(
        changes,
        /*auto_approved*/ true,
        environment_id.to_string(),
    );
    let event_ctx = ToolEventCtx::new(session, turn, &call_id, /*turn_diff_tracker*/ None);
    emitter.begin(event_ctx).await;

    let event_ctx = ToolEventCtx::new(session, turn, &call_id, /*turn_diff_tracker*/ None);
    let output = ExecToolCallOutput {
        exit_code: 0,
        ..Default::default()
    };
    let _ = emitter
        .finish(event_ctx, Ok(output), /*applied_patch_delta*/ None)
        .await;
}

#[cfg(test)]
#[path = "shell_tests.rs"]
mod tests;
