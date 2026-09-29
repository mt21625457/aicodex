use super::*;
use codex_utils_output_truncation::approx_token_count;

#[test]
fn usage_hint_is_bounded_at_the_fragment_boundary() {
    let hint =
        MultiAgentUsageHint::from_role(&crate::context::MultiAgentRoleInstructions::Configured(
            "multi-agent usage hint ".repeat(2_000),
        ));

    assert!(approx_token_count(&hint.body()) <= MULTI_AGENT_USAGE_HINT_MAX_TOKENS);
}

#[test]
fn composed_role_is_bounded_after_runtime_guidance_is_added() {
    let role = crate::context::MultiAgentRoleInstructions::Composed {
        base: "catalog role instructions ".repeat(2_000),
        marked: true,
        omit_update_plan_instructions: false,
        max_concurrency: 4,
        wait_agent_enabled: true,
        expose_model_overrides: true,
    };
    let hint = MultiAgentUsageHint::from_role(&role);

    assert!(approx_token_count(&hint.body()) <= MULTI_AGENT_USAGE_HINT_MAX_TOKENS);
    pretty_assertions::assert_eq!(
        hint.markers(),
        ("<multi_agent_role>", "</multi_agent_role>")
    );
}
