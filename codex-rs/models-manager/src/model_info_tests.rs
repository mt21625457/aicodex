use super::*;
use crate::ModelsManagerConfig;
use codex_prompts::render_model_instructions;
use codex_protocol::config_types::Personality;
use codex_protocol::openai_models::ApprovalMessages;
use codex_protocol::openai_models::AutoReviewMessages;
use codex_protocol::openai_models::CollaborationModeMessages;
use codex_protocol::openai_models::ConfirmationPolicies;
use codex_protocol::openai_models::GuardianV2ModelConfig;
use codex_protocol::openai_models::ModelTokenBudgetConfig;
use codex_protocol::openai_models::MultiAgentMessages;
use codex_protocol::openai_models::MultiAgentModeMessages;
use codex_protocol::openai_models::MultiAgentRoleMessages;
use codex_protocol::openai_models::MultiAgentToolMessages;
use codex_protocol::openai_models::PermissionMessages;
use codex_protocol::openai_models::ToolMessage;
use codex_protocol::openai_models::ToolMessages;
use pretty_assertions::assert_eq;

fn config_with_personality(personality: Option<Personality>) -> ModelsManagerConfig {
    ModelsManagerConfig {
        personality,
        ..Default::default()
    }
}

#[test]
fn base_instruction_override_is_literal_and_preserves_catalog_messages() {
    let override_instructions = "override {{ personality }}\n# Personality\nKeep me";
    let persistent_instructions = "Follow up on the active task.";
    let async_message_description = "Catalog async message description.";
    let mut model = model_info_from_slug("unknown-model");
    let approvals = ApprovalMessages {
        on_request: Some("user approvals".to_string()),
        on_request_auto_review: Some("auto approvals".to_string()),
        never: Some("never approvals".to_string()),
        unless_trusted: Some("unless-trusted approvals".to_string()),
    };
    let collaboration_modes = CollaborationModeMessages {
        default: Some("default instructions".to_string()),
        plan: Some("plan instructions".to_string()),
    };
    let auto_review = AutoReviewMessages {
        policy: Some("review policy".to_string()),
        policy_template: Some("review policy template".to_string()),
        node_repl_policy: None,
        rejection_instructions: Some("rejection instructions".to_string()),
        timeout_instructions: Some(String::new()),
    };
    let permissions = PermissionMessages {
        danger_full_access: Some("danger".to_string()),
        workspace_write: Some(String::new()),
        read_only: None,
    };
    let multi_agent = MultiAgentMessages {
        role: Some(MultiAgentRoleMessages {
            root: Some("root base".to_string()),
            subagent: Some("subagent base".to_string()),
        }),
        mode: Some(MultiAgentModeMessages {
            explicit: Some("explicit mode".to_string()),
            proactive: Some("proactive mode".to_string()),
            hint_text: Some("mode hint".to_string()),
        }),
    };
    let token_budget = ModelTokenBudgetConfig {
        enabled: false,
        use_history_notes_extension: false,
        reminder_threshold_tokens: 128,
        reminder_message_template: "budget reminder".to_string(),
        guidance_message: "budget guidance".to_string(),
        auto_compact_fallback_prompt: "compact prompt".to_string(),
        auto_compact_fallback_buffer_tokens: 64,
    };
    let guardian_v2 = GuardianV2ModelConfig {
        classifier_instructions: Some("Guardian experiment".to_string()),
        ..Default::default()
    };
    let confirmation_policies = ConfirmationPolicies {
        browser_use: Some("# Browser policy\n\n{{literal_markdown}}\n".to_string()),
        computer_use: Some("  # Native policy\r\n\n${native_markdown}\n".to_string()),
    };
    let mut messages = ModelMessages {
        content_filter_guidance: None,
        persistent_instructions: Some(persistent_instructions.to_string()),
        tools: Some(ToolMessages {
            send_user_message_async: Some(ToolMessage {
                description: Some(async_message_description.to_string()),
                ..Default::default()
            }),
            multi_agent: Some(MultiAgentToolMessages {
                spawn_agent: Some(ToolMessage {
                    description: Some("Catalog spawn description.".to_string()),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            ..Default::default()
        }),
        instructions_template: Some("template".to_string()),
        approvals: Some(approvals),
        collaboration_modes: Some(collaboration_modes),
        auto_review: Some(auto_review),
        permissions: Some(permissions),
        multi_agent: Some(multi_agent),
        token_budget: Some(token_budget),
        confirmation_policies: Some(confirmation_policies),
        guardian_v2: Some(guardian_v2),
    };
    model.model_messages = Some(messages.clone());
    let config = ModelsManagerConfig {
        base_instructions: Some(override_instructions.to_string()),
        personality: Some(Personality::None),
        ..Default::default()
    };

    let updated = with_config_overrides(model, &config);

    messages.instructions_template = Some(override_instructions.to_string());
    assert_eq!(updated.model_messages, Some(messages));
    assert_eq!(render_model_instructions(&updated), override_instructions);
}

#[test]
fn personality_none_strips_catalog_instruction_sources_through_the_next_h1() {
    let config = config_with_personality(Some(Personality::None));
    for (instructions, expected) in [
        (
            "Intro\n\n# Personality\n\nRemove me\n\n## Writing Style\n\nRemove me too\n\n# Safety\n\nKeep me",
            "Intro\n\n# Safety\n\nKeep me",
        ),
        ("Intro\n\n# Personality\n\nRemove me", "Intro\n\n"),
        (
            "Intro\n\n## Personality\n\nKeep me",
            "Intro\n\n## Personality\n\nKeep me",
        ),
        (
            "Intro\n\n# Personality \n\nKeep me",
            "Intro\n\n# Personality \n\nKeep me",
        ),
        (
            "Intro\r\n\r\n# Personality\r\n\r\nRemove me\r\n\r\n## Writing Style\r\n\r\nRemove me too\r\n\r\n# General\r\n\r\nKeep me",
            "Intro\r\n\r\n# General\r\n\r\nKeep me",
        ),
    ] {
        let mut messages = ModelMessages {
            instructions_template: Some(instructions.to_string()),
            persistent_instructions: Some(String::new()),
            tools: Some(ToolMessages {
                send_user_message_async: Some(ToolMessage {
                    description: Some(String::new()),
                    ..Default::default()
                }),
                multi_agent: Some(MultiAgentToolMessages {
                    spawn_agent: Some(ToolMessage {
                        description: Some(String::new()),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            }),
            approvals: Some(ApprovalMessages {
                on_request: Some("user approvals".to_string()),
                on_request_auto_review: None,
                never: None,
                unless_trusted: None,
            }),
            ..Default::default()
        };
        let mut model = model_info_from_slug("unknown-model");
        model.model_messages = Some(messages.clone());

        let updated = with_config_overrides(model, &config);

        messages.instructions_template = Some(expected.to_string());
        assert_eq!(updated.model_messages, Some(messages));
    }
}

#[test]
fn baked_personality_section_is_preserved_without_explicit_none() {
    let instructions = "Intro\n# Personality\nKeep me\n# General\nKeep me too";
    let configs = [
        config_with_personality(/*personality*/ None),
        config_with_personality(Some(Personality::Friendly)),
        config_with_personality(Some(Personality::Pragmatic)),
    ];

    for config in configs {
        let mut model = model_info_from_slug("unknown-model");
        model
            .model_messages
            .as_mut()
            .expect("fallback model messages")
            .instructions_template = Some(instructions.to_string());

        assert_eq!(
            render_model_instructions(&with_config_overrides(model, &config)),
            instructions
        );
    }
}

#[test]
fn explicit_empty_base_instructions_stay_empty_with_personality_none() {
    let mut model = model_info_from_slug("unknown-model");
    model
        .model_messages
        .as_mut()
        .expect("fallback model messages")
        .instructions_template = Some("Intro\n# Personality\nRemove me".to_string());
    let config = ModelsManagerConfig {
        base_instructions: Some(String::new()),
        personality: Some(Personality::None),
        ..Default::default()
    };

    let updated = with_config_overrides(model, &config);

    assert_eq!(
        updated
            .model_messages
            .as_ref()
            .and_then(|messages| messages.instructions_template.as_deref()),
        Some("")
    );
    assert_eq!(render_model_instructions(&updated), "");
}

#[test]
fn unknown_model_uses_builtin_instruction_template() {
    let model = model_info_from_slug("unknown-model");

    assert_eq!(render_model_instructions(&model), BASE_INSTRUCTIONS);
    assert!(model.used_fallback_model_metadata);
}

#[test]
fn model_context_window_override_clamps_to_max_context_window() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig {
        model_context_window: Some(500_000),
        ..Default::default()
    };

    let updated = with_config_overrides(model.clone(), &config);
    let mut expected = model;
    expected.context_window = Some(400_000);

    assert_eq!(updated, expected);
}

#[test]
fn model_context_window_uses_model_value_without_override() {
    let mut model = model_info_from_slug("unknown-model");
    model.context_window = Some(273_000);
    model.max_context_window = Some(400_000);
    let config = ModelsManagerConfig::default();

    let updated = with_config_overrides(model.clone(), &config);

    assert_eq!(updated, model);
}

#[test]
fn unknown_deepseek_models_use_large_fallback_context_window() {
    let model = model_info_from_slug("deepseek-v4-pro");

    assert_eq!(model.context_window, Some(1_048_576));
    assert_eq!(model.max_context_window, Some(1_048_576));
}

#[test]
fn unknown_k3_models_advertise_low_high_max_reasoning_levels() {
    use codex_protocol::openai_models::ReasoningEffort;
    use codex_protocol::openai_models::ReasoningEffortPreset;

    for slug in ["k3", "aicodex_gateway_claude:k3"] {
        let model = model_info_from_slug(slug);

        assert_eq!(model.context_window, Some(1_048_576));
        assert_eq!(model.max_context_window, Some(1_048_576));
        assert_eq!(model.default_reasoning_level, Some(ReasoningEffort::Max));
        assert_eq!(
            model.supported_reasoning_levels,
            vec![
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Low,
                    description: "Lighter reasoning for faster, simpler coding tasks".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::High,
                    description: "Deeper reasoning for complex engineering problems".to_string(),
                },
                ReasoningEffortPreset {
                    effort: ReasoningEffort::Max,
                    description: "Maximum reasoning depth for the hardest long-horizon tasks"
                        .to_string(),
                },
            ]
        );
    }
}

#[test]
fn openai_gpt_model_slug_recognizes_official_gpt_families() {
    assert!(is_openai_gpt_model_slug("gpt-5.5"));
    assert!(is_openai_gpt_model_slug("chatgpt-5.5"));
    assert!(is_openai_gpt_model_slug(
        "aicodex_gateway_responses:gpt-5.4"
    ));
    assert!(is_openai_gpt_model_slug("openai/gpt-5.1-codex"));
    assert!(!is_openai_gpt_model_slug("deepseek-v4-flash"));
    assert!(!is_openai_gpt_model_slug("grok-4.5"));
    assert!(!is_openai_gpt_model_slug("MiniMax-M3"));
    assert!(!is_openai_gpt_model_slug("k3"));
}

#[test]
fn deepseek_model_slug_recognizes_plain_and_prefixed_slugs() {
    assert!(is_deepseek_model_slug("deepseek-v4-pro"));
    assert!(is_deepseek_model_slug("deepseek-reasoner"));
    assert!(is_deepseek_model_slug(
        "aicodex_gateway_responses:deepseek-v4-flash"
    ));
    assert!(is_deepseek_model_slug("openai/deepseek-chat"));
    assert!(is_deepseek_model_slug("deepseek/deepseek-r1:free"));
    assert!(!is_deepseek_model_slug("deepseek-proxy:gpt-5.4"));
    assert!(!is_deepseek_model_slug("gpt-5.4"));
    assert!(!is_deepseek_model_slug("grok-4.5"));
    assert!(!is_deepseek_model_slug("k3"));
}

#[test]
fn minimax_model_slug_recognizes_plain_and_prefixed_slugs() {
    assert!(is_minimax_model_slug("MiniMax-M3"));
    assert!(is_minimax_model_slug("minimax-m3"));
    assert!(is_minimax_model_slug("minimax:MiniMax-M3"));
    assert!(is_minimax_model_slug(
        "aicodex_gateway_responses:MiniMax-M3"
    ));
    assert!(is_minimax_model_slug("openai/MiniMax-M3"));
    assert!(is_minimax_model_slug("minimax/MiniMax-M3:free"));
    assert!(!is_minimax_model_slug("minimax"));
    assert!(!is_minimax_model_slug("gpt-5.4"));
    assert!(!is_minimax_model_slug("deepseek-v4-pro"));
    assert!(!is_minimax_model_slug("grok-4.5"));
}

#[test]
fn grok_models_use_multi_agent_v2_metadata() {
    use codex_protocol::protocol::MultiAgentVersion;

    for slug in [
        "grok-4.5",
        "GROK_4",
        "aicodex_gateway_responses:grok-4",
        "xai/grok-4",
    ] {
        let model = model_info_from_slug(slug);

        assert_eq!(model.multi_agent_version, Some(MultiAgentVersion::V2));
    }
}

#[test]
fn grok_remote_metadata_without_multi_agent_version_is_upgraded() {
    let model = model_info_from_slug("grok-4.5");
    let mut remote_model = model.clone();
    remote_model.used_fallback_model_metadata = false;
    remote_model.multi_agent_version = None;

    let updated = with_config_overrides(remote_model, &ModelsManagerConfig::default());

    let mut expected = model;
    expected.used_fallback_model_metadata = false;
    assert_eq!(updated, expected);
}
