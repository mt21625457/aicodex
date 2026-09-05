use codex_features::Feature;
use codex_features::Features;
use codex_protocol::config_types::ModeKind;
use pretty_assertions::assert_eq;

use super::*;

#[test]
fn request_user_input_modes_follow_default_mode_feature() {
    let mut features = Features::with_defaults();
    features.disable(Feature::DefaultModeRequestUserInput);
    assert_eq!(
        request_user_input_available_modes(&features),
        vec![ModeKind::Plan]
    );

    features.enable(Feature::DefaultModeRequestUserInput);
    assert_eq!(
        request_user_input_available_modes(&features),
        vec![ModeKind::Default, ModeKind::Plan]
    );
}

#[test]
fn unified_exec_shell_mode_uses_zsh_fork_only_when_all_inputs_match() {
    let executable = std::env::current_exe().expect("current exe path");
    let required_features = [
        Feature::ShellTool,
        Feature::UnifiedExec,
        Feature::ShellZshFork,
        Feature::UnifiedExecZshFork,
    ];
    let mut features = Features::with_defaults();
    for feature in required_features {
        features.enable(feature);
    }
    let expected = if cfg!(unix) {
        UnifiedExecShellMode::ZshFork(ZshForkConfig {
            shell_zsh_path: AbsolutePathBuf::try_from(executable.clone()).unwrap(),
            main_execve_wrapper_exe: AbsolutePathBuf::try_from(executable.clone()).unwrap(),
        })
    } else {
        UnifiedExecShellMode::Direct
    };
    assert_eq!(
        UnifiedExecShellMode::for_session(
            &features,
            ToolUserShellType::Zsh,
            Some(&executable),
            Some(&executable),
        ),
        expected
    );
    for feature in required_features {
        let mut disabled = features.clone();
        disabled.disable(feature);
        assert_eq!(
            UnifiedExecShellMode::for_session(
                &disabled,
                ToolUserShellType::Zsh,
                Some(&executable),
                Some(&executable),
            ),
            UnifiedExecShellMode::Direct
        );
    }
    assert_eq!(
        UnifiedExecShellMode::for_session(
            &features,
            ToolUserShellType::Bash,
            Some(&executable),
            Some(&executable),
        ),
        UnifiedExecShellMode::Direct
    );
}
