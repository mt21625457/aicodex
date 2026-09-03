use crate::sandboxing::SandboxPermissions;
use crate::shell::Shell;
use crate::shell::ShellType;
use crate::shell::get_shell_by_model_provided_path;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::hook_names::HookToolName;
use crate::tools::registry::PostToolUsePayload;
use codex_exec_server::Environment;
use codex_protocol::models::AdditionalPermissionProfile;
use codex_tools::UnifiedExecShellMode;
use serde::Deserialize;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(test)]
use crate::tools::handlers::parse_arguments;

mod exec_command;
mod write_stdin;

pub use exec_command::ExecCommandHandler;
pub(crate) use exec_command::ExecCommandHandlerOptions;
pub use write_stdin::WriteStdinHandler;

use crate::function_tool::FunctionCallError;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::PreToolUsePayload;
use codex_tools::CLAUDE_BASH_TOOL_NAME;
use codex_tools::JsonSchema;
use codex_tools::ResponsesApiTool;
use codex_tools::ToolExecutor;
use codex_tools::ToolName;
use codex_tools::ToolSpec;

pub(crate) struct ClaudeBashHandler {
    inner: ExecCommandHandler,
}

impl ClaudeBashHandler {
    pub(crate) fn new(options: ExecCommandHandlerOptions) -> Self {
        Self {
            inner: ExecCommandHandler::new(options),
        }
    }
}

impl ToolExecutor<ToolInvocation> for ClaudeBashHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(CLAUDE_BASH_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        ToolSpec::Function(ResponsesApiTool {
            name: CLAUDE_BASH_TOOL_NAME.to_string(),
            description: "Claude native bash runtime.".to_string(),
            strict: false,
            defer_loading: None,
            parameters: JsonSchema::default(),
            output_schema: None,
        })
    }

    fn handle<'a>(&'a self, mut invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'a>
    where
        ToolInvocation: 'a,
    {
        Box::pin(async move {
            let ToolPayload::Function { arguments } = &invocation.payload else {
                return Err(FunctionCallError::RespondToModel(
                    "Claude bash received an unsupported payload".to_string(),
                ));
            };
            let mut arguments: Value = serde_json::from_str(arguments).map_err(|err| {
                FunctionCallError::RespondToModel(format!(
                    "failed to parse Claude bash arguments: {err}"
                ))
            })?;
            let Value::Object(arguments) = &mut arguments else {
                return Err(FunctionCallError::RespondToModel(
                    "Claude bash arguments must be an object".to_string(),
                ));
            };
            let Some(command) = arguments.remove("command") else {
                return Err(FunctionCallError::RespondToModel(
                    "Claude bash requires a string `command`".to_string(),
                ));
            };
            if !command.is_string() {
                return Err(FunctionCallError::RespondToModel(
                    "Claude bash requires a string `command`".to_string(),
                ));
            }
            arguments.insert("cmd".to_string(), command);
            invocation.payload = ToolPayload::Function {
                arguments: serde_json::to_string(arguments).map_err(|err| {
                    FunctionCallError::RespondToModel(format!(
                        "failed to serialize Claude bash arguments: {err}"
                    ))
                })?,
            };
            self.inner.handle(invocation).await
        })
    }
}

impl CoreToolRuntime for ClaudeBashHandler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }

    fn pre_tool_use_payload(&self, invocation: &ToolInvocation) -> Option<PreToolUsePayload> {
        let ToolPayload::Function { arguments } = &invocation.payload else {
            return None;
        };
        serde_json::from_str::<Value>(arguments)
            .ok()
            .and_then(|arguments| arguments.get("command").cloned())
            .map(|command| PreToolUsePayload {
                tool_name: HookToolName::bash(),
                tool_input: serde_json::json!({ "command": command }),
            })
    }
}

#[derive(Debug, Deserialize)]
pub(crate) struct ExecCommandArgs {
    pub(crate) cmd: String,
    #[serde(default)]
    shell: Option<String>,
    #[serde(default)]
    login: Option<bool>,
    #[serde(default = "default_tty")]
    tty: bool,
    #[serde(default = "default_exec_yield_time_ms")]
    yield_time_ms: u64,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    max_output_tokens: Option<usize>,
    #[serde(default)]
    sandbox_permissions: Option<SandboxPermissions>,
    #[serde(default)]
    additional_permissions: Option<AdditionalPermissionProfile>,
    #[serde(default)]
    justification: Option<String>,
    #[serde(default)]
    prefix_rule: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
struct ExecCommandEnvironmentArgs {
    #[serde(default)]
    environment_id: Option<String>,
    // Keep this raw until after environment selection; relative paths must be
    // resolved against the selected environment cwd, not the process cwd.
    #[serde(default)]
    workdir: Option<String>,
}

fn default_exec_yield_time_ms() -> u64 {
    10_000
}

fn default_write_stdin_yield_time_ms() -> u64 {
    250
}

fn default_tty() -> bool {
    false
}

#[derive(Debug)]
pub(crate) struct ResolvedCommand {
    pub(crate) command: Vec<String>,
    pub(crate) shell_type: ShellType,
}

fn post_unified_exec_tool_use_payload(
    invocation: &ToolInvocation,
    result: &dyn ToolOutput,
) -> Option<PostToolUsePayload> {
    let ToolPayload::Function { .. } = &invocation.payload else {
        return None;
    };

    let tool_input = result.post_tool_use_input(&invocation.payload)?;
    let tool_use_id = result.post_tool_use_id(&invocation.call_id);
    let tool_response = result.post_tool_use_response(&tool_use_id, &invocation.payload)?;
    Some(PostToolUsePayload {
        tool_name: HookToolName::bash(),
        tool_use_id,
        tool_input,
        tool_response,
    })
}

pub(crate) fn get_command(
    args: &ExecCommandArgs,
    session_shell: Arc<Shell>,
    shell_mode: &UnifiedExecShellMode,
    allow_login_shell: bool,
) -> Result<ResolvedCommand, String> {
    let use_login_shell = match args.login {
        Some(true) if !allow_login_shell => {
            return Err(
                "login shell is disabled by config; omit `login` or set it to false.".to_string(),
            );
        }
        Some(use_login_shell) => use_login_shell,
        None => allow_login_shell,
    };

    match shell_mode {
        UnifiedExecShellMode::Direct => {
            let model_shell = args
                .shell
                .as_ref()
                .map(|shell_str| get_shell_by_model_provided_path(&PathBuf::from(shell_str)));
            let shell = model_shell.as_ref().unwrap_or(session_shell.as_ref());
            Ok(ResolvedCommand {
                command: shell.derive_exec_args(&args.cmd, use_login_shell),
                shell_type: shell.shell_type,
            })
        }
        UnifiedExecShellMode::ZshFork(zsh_fork_config) => {
            if args.shell.is_some() {
                return Err(
                    "`shell` is not supported for local zsh-fork exec; omit `shell` to use zsh-fork, or target a remote environment where `shell` is supported.".to_string(),
                );
            }

            Ok(ResolvedCommand {
                command: vec![
                    zsh_fork_config.shell_zsh_path.to_string_lossy().to_string(),
                    if use_login_shell { "-lc" } else { "-c" }.to_string(),
                    args.cmd.clone(),
                ],
                shell_type: ShellType::Zsh,
            })
        }
    }
}

pub(crate) fn shell_mode_for_environment(
    turn_shell_mode: &UnifiedExecShellMode,
    environment: &Environment,
) -> UnifiedExecShellMode {
    if environment.is_remote() {
        UnifiedExecShellMode::Direct
    } else {
        turn_shell_mode.clone()
    }
}

#[cfg(test)]
#[path = "unified_exec_tests.rs"]
mod tests;
