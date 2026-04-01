use std::str::FromStr;

use codex_utils_fuzzy_match::fuzzy_match;
use strum::IntoEnumIterator;
use strum_macros::AsRefStr;
use strum_macros::EnumIter;
use strum_macros::EnumString;
use strum_macros::IntoStaticStr;

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, EnumString, EnumIter, AsRefStr, IntoStaticStr,
)]
#[strum(serialize_all = "kebab-case")]
pub enum SlashCommand {
    Model,
    Fast,
    Approvals,
    Permissions,
    #[strum(serialize = "setup-default-sandbox")]
    ElevateSandbox,
    #[strum(serialize = "sandbox-add-read-dir")]
    SandboxReadRoot,
    Experimental,
    Skills,
    Review,
    Rename,
    New,
    Resume,
    Fork,
    Init,
    Compact,
    Plan,
    Collab,
    Agent,
    Diff,
    Copy,
    Mention,
    Status,
    DebugConfig,
    Title,
    Statusline,
    Theme,
    Mcp,
    Apps,
    Plugins,
    Logout,
    Quit,
    Exit,
    Feedback,
    Rollout,
    Ps,
    #[strum(to_string = "stop", serialize = "clean")]
    Stop,
    Clear,
    Personality,
    Realtime,
    Settings,
    TestApproval,
    #[strum(serialize = "subagents")]
    MultiAgents,
    #[strum(serialize = "debug-m-drop")]
    MemoryDrop,
    #[strum(serialize = "debug-m-update")]
    MemoryUpdate,
}

impl SlashCommand {
    pub fn description(self) -> &'static str {
        match self {
            SlashCommand::Feedback => "send logs to maintainers",
            SlashCommand::New => "start a new chat during a conversation",
            SlashCommand::Init => "create an AGENTS.md file with instructions for Codex",
            SlashCommand::Compact => "summarize conversation to prevent hitting the context limit",
            SlashCommand::Review => "review my current changes and find issues",
            SlashCommand::Rename => "rename the current thread",
            SlashCommand::Resume => "resume a saved chat",
            SlashCommand::Clear => "clear the terminal and start a new chat",
            SlashCommand::Fork => "fork the current chat",
            SlashCommand::Quit | SlashCommand::Exit => "exit Codex",
            SlashCommand::Diff => "show git diff (including untracked files)",
            SlashCommand::Copy => "copy the latest Codex output to your clipboard",
            SlashCommand::Mention => "mention a file",
            SlashCommand::Skills => "use skills to improve how Codex performs specific tasks",
            SlashCommand::Status => "show current session configuration and token usage",
            SlashCommand::DebugConfig => "show config layers and requirement sources for debugging",
            SlashCommand::Title => "configure which items appear in the terminal title",
            SlashCommand::Statusline => "configure which items appear in the status line",
            SlashCommand::Theme => "choose a syntax highlighting theme",
            SlashCommand::Ps => "list background terminals",
            SlashCommand::Stop => "stop all background terminals",
            SlashCommand::MemoryDrop => "DO NOT USE",
            SlashCommand::MemoryUpdate => "DO NOT USE",
            SlashCommand::Model => "choose what model and reasoning effort to use",
            SlashCommand::Fast => "toggle Fast mode to enable fastest inference at 2X plan usage",
            SlashCommand::Personality => "choose a communication style for Codex",
            SlashCommand::Realtime => "toggle realtime voice mode (experimental)",
            SlashCommand::Settings => "configure realtime microphone/speaker",
            SlashCommand::Plan => "switch to Plan mode",
            SlashCommand::Collab => "change collaboration mode (experimental)",
            SlashCommand::Agent | SlashCommand::MultiAgents => "switch the active agent thread",
            SlashCommand::Approvals => "choose what Codex is allowed to do",
            SlashCommand::Permissions => "choose what Codex is allowed to do",
            SlashCommand::ElevateSandbox => "set up elevated agent sandbox",
            SlashCommand::SandboxReadRoot => {
                "let sandbox read a directory: /sandbox-add-read-dir <absolute_path>"
            }
            SlashCommand::Experimental => "toggle experimental features",
            SlashCommand::Mcp => "list configured MCP tools",
            SlashCommand::Apps => "manage apps",
            SlashCommand::Plugins => "browse plugins",
            SlashCommand::Logout => "log out of Codex",
            SlashCommand::Rollout => "print the rollout file path",
            SlashCommand::TestApproval => "test approval request",
        }
    }

    pub fn command(self) -> &'static str {
        self.into()
    }

    pub fn supports_inline_args(self) -> bool {
        matches!(
            self,
            SlashCommand::Review
                | SlashCommand::Rename
                | SlashCommand::Plan
                | SlashCommand::Fast
                | SlashCommand::SandboxReadRoot
        )
    }

    pub fn available_during_task(self) -> bool {
        match self {
            SlashCommand::New
            | SlashCommand::Resume
            | SlashCommand::Fork
            | SlashCommand::Init
            | SlashCommand::Compact
            | SlashCommand::Model
            | SlashCommand::Fast
            | SlashCommand::Personality
            | SlashCommand::Approvals
            | SlashCommand::Permissions
            | SlashCommand::ElevateSandbox
            | SlashCommand::SandboxReadRoot
            | SlashCommand::Experimental
            | SlashCommand::Review
            | SlashCommand::Plan
            | SlashCommand::Clear
            | SlashCommand::Logout
            | SlashCommand::MemoryDrop
            | SlashCommand::MemoryUpdate => false,
            SlashCommand::Diff
            | SlashCommand::Copy
            | SlashCommand::Rename
            | SlashCommand::Mention
            | SlashCommand::Skills
            | SlashCommand::Status
            | SlashCommand::DebugConfig
            | SlashCommand::Ps
            | SlashCommand::Stop
            | SlashCommand::Mcp
            | SlashCommand::Apps
            | SlashCommand::Plugins
            | SlashCommand::Feedback
            | SlashCommand::Quit
            | SlashCommand::Exit => true,
            SlashCommand::Rollout => true,
            SlashCommand::TestApproval => true,
            SlashCommand::Realtime => true,
            SlashCommand::Settings => true,
            SlashCommand::Collab => true,
            SlashCommand::Agent | SlashCommand::MultiAgents => true,
            SlashCommand::Statusline => false,
            SlashCommand::Theme => false,
            SlashCommand::Title => false,
        }
    }

    fn is_visible(self) -> bool {
        match self {
            SlashCommand::SandboxReadRoot => cfg!(target_os = "windows"),
            SlashCommand::Copy => !cfg!(target_os = "android"),
            SlashCommand::Rollout | SlashCommand::TestApproval => cfg!(debug_assertions),
            _ => true,
        }
    }
}

pub fn built_in_slash_commands() -> Vec<(&'static str, SlashCommand)> {
    SlashCommand::iter()
        .filter(|command| command.is_visible())
        .map(|c| (c.command(), c))
        .collect()
}

#[derive(Clone, Copy, Debug, Default)]
pub struct BuiltinCommandFlags {
    pub collaboration_modes_enabled: bool,
    pub connectors_enabled: bool,
    pub plugins_command_enabled: bool,
    pub fast_command_enabled: bool,
    pub personality_command_enabled: bool,
    pub realtime_conversation_enabled: bool,
    pub audio_device_selection_enabled: bool,
    pub allow_elevate_sandbox: bool,
}

pub fn builtins_for_input(flags: BuiltinCommandFlags) -> Vec<(&'static str, SlashCommand)> {
    built_in_slash_commands()
        .into_iter()
        .filter(|(_, cmd)| flags.allow_elevate_sandbox || *cmd != SlashCommand::ElevateSandbox)
        .filter(|(_, cmd)| {
            flags.collaboration_modes_enabled
                || !matches!(*cmd, SlashCommand::Collab | SlashCommand::Plan)
        })
        .filter(|(_, cmd)| flags.connectors_enabled || *cmd != SlashCommand::Apps)
        .filter(|(_, cmd)| flags.plugins_command_enabled || *cmd != SlashCommand::Plugins)
        .filter(|(_, cmd)| flags.fast_command_enabled || *cmd != SlashCommand::Fast)
        .filter(|(_, cmd)| flags.personality_command_enabled || *cmd != SlashCommand::Personality)
        .filter(|(_, cmd)| flags.realtime_conversation_enabled || *cmd != SlashCommand::Realtime)
        .filter(|(_, cmd)| flags.audio_device_selection_enabled || *cmd != SlashCommand::Settings)
        .collect()
}

pub fn find_builtin_command(name: &str, flags: BuiltinCommandFlags) -> Option<SlashCommand> {
    let cmd = SlashCommand::from_str(name).ok()?;
    builtins_for_input(flags)
        .into_iter()
        .any(|(_, visible_cmd)| visible_cmd == cmd)
        .then_some(cmd)
}

pub fn parse_builtin_command(name: &str) -> Option<SlashCommand> {
    SlashCommand::from_str(name).ok()
}

pub fn has_builtin_prefix(name: &str, flags: BuiltinCommandFlags) -> bool {
    builtins_for_input(flags)
        .into_iter()
        .any(|(command_name, _)| fuzzy_match(command_name, name).is_some())
}

pub fn parse_slash_name(line: &str) -> Option<(&str, &str, usize)> {
    let stripped = line.strip_prefix('/')?;
    let mut name_end_in_stripped = stripped.len();
    for (idx, ch) in stripped.char_indices() {
        if ch.is_whitespace() {
            name_end_in_stripped = idx;
            break;
        }
    }
    let name = &stripped[..name_end_in_stripped];
    if name.is_empty() {
        return None;
    }
    let rest_untrimmed = &stripped[name_end_in_stripped..];
    let rest = rest_untrimmed.trim_start();
    let rest_start_in_stripped = name_end_in_stripped + (rest_untrimmed.len() - rest.len());
    let rest_offset = rest_start_in_stripped + 1;
    Some((name, rest, rest_offset))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientActionKind {
    StartNewThread,
    OpenResumePicker,
    ForkCurrentThread,
    OpenModelPicker,
    OpenPermissionsPanel,
    OpenPersonalityPanel,
    SwitchToPlanMode,
    OpenCollaborationModePicker,
    OpenMcpOverview,
    OpenConnectorsOverview,
    OpenPluginsOverview,
    OpenStatusView,
    OpenDiffView,
}

impl ClientActionKind {
    pub fn action_type(self) -> &'static str {
        match self {
            ClientActionKind::StartNewThread => "start_new_thread",
            ClientActionKind::OpenResumePicker => "open_resume_picker",
            ClientActionKind::ForkCurrentThread => "fork_current_thread",
            ClientActionKind::OpenModelPicker => "open_model_picker",
            ClientActionKind::OpenPermissionsPanel => "open_permissions_panel",
            ClientActionKind::OpenPersonalityPanel => "open_personality_panel",
            ClientActionKind::SwitchToPlanMode => "switch_to_plan_mode",
            ClientActionKind::OpenCollaborationModePicker => "open_collaboration_mode_picker",
            ClientActionKind::OpenMcpOverview => "open_mcp_overview",
            ClientActionKind::OpenConnectorsOverview => "open_connectors_overview",
            ClientActionKind::OpenPluginsOverview => "open_plugins_overview",
            ClientActionKind::OpenStatusView => "open_status_view",
            ClientActionKind::OpenDiffView => "open_diff_view",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FastModeState {
    On,
    Off,
    Status,
}

impl FastModeState {
    pub fn as_str(self) -> &'static str {
        match self {
            FastModeState::On => "on",
            FastModeState::Off => "off",
            FastModeState::Status => "status",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayActionKind {
    CompactThread,
    RenameThread { name: String },
    StartReview { instructions: String },
    SetFastMode { state: FastModeState },
}

impl GatewayActionKind {
    pub fn action_type(&self) -> &'static str {
        match self {
            GatewayActionKind::CompactThread => "compact_thread",
            GatewayActionKind::RenameThread { .. } => "rename_thread",
            GatewayActionKind::StartReview { .. } => "start_review",
            GatewayActionKind::SetFastMode { .. } => "set_fast_mode",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SlashDispatchResult {
    PassthroughMessage,
    ClientAction {
        command: SlashCommand,
        action: ClientActionKind,
    },
    GatewayAction {
        command: SlashCommand,
        action: GatewayActionKind,
    },
    Unsupported {
        command: SlashCommand,
        message: String,
    },
    InvalidArgs {
        command: SlashCommand,
        message: String,
    },
}

pub fn dispatch_for_web(
    text: &str,
    flags: BuiltinCommandFlags,
    task_running: bool,
) -> SlashDispatchResult {
    let Some((name, rest, _rest_offset)) = parse_slash_name(text.trim_start()) else {
        return SlashDispatchResult::PassthroughMessage;
    };
    let Some(command) = parse_builtin_command(name) else {
        return SlashDispatchResult::PassthroughMessage;
    };

    if let Some(message) = unsupported_message_for_hidden_command(command, flags) {
        return SlashDispatchResult::Unsupported { command, message };
    }

    if task_running && !command.available_during_task() {
        return SlashDispatchResult::Unsupported {
            command,
            message: format!(
                "`/{}` is disabled while a task is in progress.",
                command.command()
            ),
        };
    }

    match command {
        SlashCommand::New => client_action(command, ClientActionKind::StartNewThread),
        SlashCommand::Resume => client_action(command, ClientActionKind::OpenResumePicker),
        SlashCommand::Fork => client_action(command, ClientActionKind::ForkCurrentThread),
        SlashCommand::Model => reject_inline_args(command, rest, ClientActionKind::OpenModelPicker),
        SlashCommand::Approvals | SlashCommand::Permissions => {
            reject_inline_args(command, rest, ClientActionKind::OpenPermissionsPanel)
        }
        SlashCommand::Personality => {
            reject_inline_args(command, rest, ClientActionKind::OpenPersonalityPanel)
        }
        SlashCommand::Plan => reject_inline_args(command, rest, ClientActionKind::SwitchToPlanMode),
        SlashCommand::Collab => {
            reject_inline_args(command, rest, ClientActionKind::OpenCollaborationModePicker)
        }
        SlashCommand::Mcp => reject_inline_args(command, rest, ClientActionKind::OpenMcpOverview),
        SlashCommand::Apps => {
            reject_inline_args(command, rest, ClientActionKind::OpenConnectorsOverview)
        }
        SlashCommand::Plugins => {
            reject_inline_args(command, rest, ClientActionKind::OpenPluginsOverview)
        }
        SlashCommand::Status => reject_inline_args(command, rest, ClientActionKind::OpenStatusView),
        SlashCommand::Diff => reject_inline_args(command, rest, ClientActionKind::OpenDiffView),
        SlashCommand::Compact => reject_inline_gateway_action(command, rest),
        SlashCommand::Rename => {
            let trimmed = rest.trim();
            if trimmed.is_empty() {
                return SlashDispatchResult::InvalidArgs {
                    command,
                    message: "Usage: /rename <name>".to_string(),
                };
            }
            SlashDispatchResult::GatewayAction {
                command,
                action: GatewayActionKind::RenameThread {
                    name: trimmed.to_string(),
                },
            }
        }
        SlashCommand::Review => {
            let trimmed = rest.trim();
            if trimmed.is_empty() {
                return SlashDispatchResult::InvalidArgs {
                    command,
                    message: "Usage: /review <instructions>".to_string(),
                };
            }
            SlashDispatchResult::GatewayAction {
                command,
                action: GatewayActionKind::StartReview {
                    instructions: trimmed.to_string(),
                },
            }
        }
        SlashCommand::Fast => {
            let trimmed = rest.trim();
            let state = match trimmed {
                "" | "status" => FastModeState::Status,
                "on" => FastModeState::On,
                "off" => FastModeState::Off,
                _ => {
                    return SlashDispatchResult::InvalidArgs {
                        command,
                        message: "Usage: /fast [on|off|status]".to_string(),
                    };
                }
            };
            SlashDispatchResult::GatewayAction {
                command,
                action: GatewayActionKind::SetFastMode { state },
            }
        }
        SlashCommand::Theme
        | SlashCommand::Statusline
        | SlashCommand::Quit
        | SlashCommand::Exit
        | SlashCommand::Clear
        | SlashCommand::Realtime
        | SlashCommand::Settings
        | SlashCommand::Init
        | SlashCommand::Logout
        | SlashCommand::Copy
        | SlashCommand::Mention
        | SlashCommand::Skills
        | SlashCommand::DebugConfig
        | SlashCommand::Title
        | SlashCommand::Ps
        | SlashCommand::Stop
        | SlashCommand::Feedback
        | SlashCommand::Rollout
        | SlashCommand::TestApproval
        | SlashCommand::ElevateSandbox
        | SlashCommand::SandboxReadRoot
        | SlashCommand::Experimental
        | SlashCommand::Agent
        | SlashCommand::MultiAgents
        | SlashCommand::MemoryDrop
        | SlashCommand::MemoryUpdate => SlashDispatchResult::Unsupported {
            command,
            message: format!("The web client does not support `/{}`.", command.command()),
        },
    }
}

fn unsupported_message_for_hidden_command(
    command: SlashCommand,
    flags: BuiltinCommandFlags,
) -> Option<String> {
    match command {
        SlashCommand::Plan | SlashCommand::Collab if !flags.collaboration_modes_enabled => {
            Some("The current runtime does not expose collaboration modes.".to_string())
        }
        SlashCommand::Apps if !flags.connectors_enabled => {
            Some("The current runtime does not expose `/apps`.".to_string())
        }
        SlashCommand::Plugins if !flags.plugins_command_enabled => {
            Some("The current runtime does not expose `/plugins`.".to_string())
        }
        SlashCommand::Fast if !flags.fast_command_enabled => {
            Some("The current runtime does not expose `/fast`.".to_string())
        }
        SlashCommand::Personality if !flags.personality_command_enabled => {
            Some("The current runtime does not expose `/personality`.".to_string())
        }
        SlashCommand::Realtime if !flags.realtime_conversation_enabled => {
            Some("The current runtime does not expose `/realtime`.".to_string())
        }
        SlashCommand::Settings if !flags.audio_device_selection_enabled => {
            Some("The current runtime does not expose `/settings`.".to_string())
        }
        SlashCommand::ElevateSandbox if !flags.allow_elevate_sandbox => {
            Some("The current runtime does not expose `/setup-default-sandbox`.".to_string())
        }
        _ => None,
    }
}

fn client_action(command: SlashCommand, action: ClientActionKind) -> SlashDispatchResult {
    SlashDispatchResult::ClientAction { command, action }
}

fn reject_inline_args(
    command: SlashCommand,
    rest: &str,
    action: ClientActionKind,
) -> SlashDispatchResult {
    if !rest.is_empty() {
        return SlashDispatchResult::InvalidArgs {
            command,
            message: format!("`/{}` does not accept inline arguments.", command.command()),
        };
    }
    client_action(command, action)
}

fn reject_inline_gateway_action(command: SlashCommand, rest: &str) -> SlashDispatchResult {
    if !rest.is_empty() {
        return SlashDispatchResult::InvalidArgs {
            command,
            message: format!("`/{}` does not accept inline arguments.", command.command()),
        };
    }
    SlashDispatchResult::GatewayAction {
        command,
        action: GatewayActionKind::CompactThread,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn all_enabled_flags() -> BuiltinCommandFlags {
        BuiltinCommandFlags {
            collaboration_modes_enabled: true,
            connectors_enabled: true,
            plugins_command_enabled: true,
            fast_command_enabled: true,
            personality_command_enabled: true,
            realtime_conversation_enabled: true,
            audio_device_selection_enabled: true,
            allow_elevate_sandbox: true,
        }
    }

    #[test]
    fn clean_alias_parses_to_stop_command() {
        assert_eq!(SlashCommand::from_str("clean"), Ok(SlashCommand::Stop));
    }

    #[test]
    fn debug_command_still_resolves_for_dispatch() {
        let cmd = find_builtin_command("debug-config", all_enabled_flags());
        assert_eq!(cmd, Some(SlashCommand::DebugConfig));
    }

    #[test]
    fn parse_slash_name_extracts_name_rest_and_offset() {
        let parsed = parse_slash_name("/review auth flow");
        assert_eq!(parsed, Some(("review", "auth flow", 8)));
    }

    #[test]
    fn dispatch_for_web_rejects_hidden_connectors_command_explicitly() {
        let result = dispatch_for_web(
            "/apps",
            BuiltinCommandFlags {
                connectors_enabled: false,
                ..Default::default()
            },
            false,
        );

        match result {
            SlashDispatchResult::Unsupported { command, .. } => {
                assert_eq!(command, SlashCommand::Apps);
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn dispatch_for_web_extracts_review_instructions() {
        let result = dispatch_for_web("/review auth flow", all_enabled_flags(), false);

        match result {
            SlashDispatchResult::GatewayAction { command, action } => {
                assert_eq!(command, SlashCommand::Review);
                assert_eq!(
                    action,
                    GatewayActionKind::StartReview {
                        instructions: "auth flow".to_string(),
                    }
                );
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn dispatch_for_web_extracts_rename_name() {
        let result = dispatch_for_web("/rename incident triage", all_enabled_flags(), false);

        match result {
            SlashDispatchResult::GatewayAction { command, action } => {
                assert_eq!(command, SlashCommand::Rename);
                assert_eq!(
                    action,
                    GatewayActionKind::RenameThread {
                        name: "incident triage".to_string(),
                    }
                );
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }
}
