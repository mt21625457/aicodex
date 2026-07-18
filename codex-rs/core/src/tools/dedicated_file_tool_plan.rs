use std::sync::Arc;

use codex_config::config_toml::ChatFileToolMode;
use codex_features::ClaudeFileToolMode;
use codex_features::Feature;
use codex_model_provider_info::WireApi;
use codex_tools::ToolEnvironmentMode;

use crate::claude::ClaudeProviderCompat;
use crate::session::turn_context::TurnContext;
use crate::tools::handlers::ApplyPatchHandler;
use crate::tools::handlers::ClaudeTextEditorHandler;
use crate::tools::handlers::EditFileHandler;
use crate::tools::handlers::ReadFileHandler;
use crate::tools::handlers::WriteFileHandler;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExposure;
use crate::tools::registry::override_tool_exposure;

pub(crate) fn model_visible(
    turn_context: &TurnContext,
    environment_mode: ToolEnvironmentMode,
) -> bool {
    if !environment_mode.has_environment()
        || !turn_context
            .config
            .features
            .enabled(Feature::DedicatedFileTools)
    {
        return false;
    }
    match turn_context.provider.info().wire_api {
        WireApi::Chat => !matches!(
            turn_context.config.chat_file_tool_mode,
            ChatFileToolMode::Legacy
        ),
        WireApi::Claude => claude_uses_dedicated_file_tools(turn_context),
        WireApi::Responses => false,
    }
}

pub(crate) fn planned_runtimes(
    turn_context: &TurnContext,
    environment_mode: ToolEnvironmentMode,
    apply_patch_available: bool,
) -> Vec<Arc<dyn CoreToolRuntime>> {
    if !environment_mode.has_environment() {
        return Vec::new();
    }
    let gate_enabled = turn_context
        .config
        .features
        .enabled(Feature::DedicatedFileTools);
    let chat_file_tool_mode = if turn_context.provider.info().wire_api == WireApi::Chat {
        turn_context.config.chat_file_tool_mode
    } else {
        ChatFileToolMode::Legacy
    };
    let dedicated_chat_mode =
        gate_enabled && !matches!(chat_file_tool_mode, ChatFileToolMode::Legacy);
    let dedicated_claude_mode = gate_enabled && claude_uses_dedicated_file_tools(turn_context);
    let dedicated_mode = dedicated_chat_mode || dedicated_claude_mode;
    if !apply_patch_available && !dedicated_mode {
        return Vec::new();
    }

    let include_environment_id = matches!(environment_mode, ToolEnvironmentMode::Multiple);
    let hide_apply_patch = (dedicated_chat_mode
        && matches!(chat_file_tool_mode, ChatFileToolMode::Dedicated))
        || (dedicated_claude_mode
            && !matches!(
                turn_context.config.claude_file_tool_mode,
                ClaudeFileToolMode::DedicatedWithApplyPatch
            ));
    let apply_patch_exposure = if hide_apply_patch {
        ToolExposure::Hidden
    } else {
        ToolExposure::Direct
    };
    let mut runtimes = vec![
        override_tool_exposure(
            Arc::new(ApplyPatchHandler::new(include_environment_id)),
            apply_patch_exposure,
        ),
        override_tool_exposure(
            Arc::new(ClaudeTextEditorHandler::new(include_environment_id)),
            ToolExposure::Hidden,
        ),
    ];
    if dedicated_mode {
        let state = Arc::clone(&turn_context.file_tool_state);
        runtimes.extend([
            Arc::new(ReadFileHandler::new(
                Arc::clone(&state),
                include_environment_id,
            )) as Arc<dyn CoreToolRuntime>,
            Arc::new(EditFileHandler::new(
                Arc::clone(&state),
                include_environment_id,
            )),
            Arc::new(WriteFileHandler::new(state, include_environment_id)),
        ]);
    }
    runtimes
}

pub(crate) fn claude_uses_dedicated_file_tools(turn_context: &TurnContext) -> bool {
    if turn_context.provider.info().wire_api != WireApi::Claude {
        return false;
    }
    let provider = turn_context.provider.info();
    let provider_compat = crate::claude::provider_compat_for_provider(
        &provider.name,
        provider.base_url.as_deref(),
        Some(&turn_context.model_info.slug),
    );
    claude_policy_uses_dedicated_file_tools(
        true,
        turn_context.config.claude_file_tool_mode,
        provider_compat,
        &turn_context.model_info.slug,
    )
}

pub(crate) fn claude_policy_uses_dedicated_file_tools(
    gate_enabled: bool,
    mode: ClaudeFileToolMode,
    provider_compat: ClaudeProviderCompat,
    model_slug: &str,
) -> bool {
    if !gate_enabled {
        return false;
    }
    match mode {
        ClaudeFileToolMode::Dedicated | ClaudeFileToolMode::DedicatedWithApplyPatch => true,
        ClaudeFileToolMode::Auto => {
            provider_compat == ClaudeProviderCompat::Compatible
                || crate::claude::claude_model_is_kimi_k3(model_slug)
        }
    }
}

/// True when `tools` contains the first-party dedicated file tool declaration for `name`.
///
/// Same-named third-party/dynamic tools are rejected so guidance cannot advertise an
/// unsandboxed substitute as the Codex file-IO surface.
pub(crate) fn has_first_party_dedicated_file_tool(
    tools: &[codex_tools::ToolSpec],
    name: &str,
) -> bool {
    let expected_description = match name {
        "read_file" => "Read a bounded text file through Codex's filesystem layer.",
        "edit_file" => "Replace exact text in a previously read text file.",
        "write_file" => "Create or overwrite a bounded text file through Codex's filesystem layer.",
        _ => return false,
    };
    tools.iter().any(|tool| match tool {
        codex_tools::ToolSpec::Function(tool) => {
            tool.name == name && tool.description == expected_description
        }
        _ => false,
    })
}
