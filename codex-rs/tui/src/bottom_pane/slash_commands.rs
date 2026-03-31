pub(crate) use codex_slash_dispatch::BuiltinCommandFlags;
pub(crate) use codex_slash_dispatch::builtins_for_input;
pub(crate) use codex_slash_dispatch::find_builtin_command;
pub(crate) use codex_slash_dispatch::has_builtin_prefix;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::slash_command::SlashCommand;
    use pretty_assertions::assert_eq;

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
    fn debug_command_still_resolves_for_dispatch() {
        let cmd = find_builtin_command("debug-config", all_enabled_flags());
        assert_eq!(cmd, Some(SlashCommand::DebugConfig));
    }

    #[test]
    fn clear_command_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("clear", all_enabled_flags()),
            Some(SlashCommand::Clear)
        );
    }

    #[test]
    fn stop_command_resolves_for_dispatch() {
        assert_eq!(
            find_builtin_command("stop", all_enabled_flags()),
            Some(SlashCommand::Stop)
        );
    }
}
