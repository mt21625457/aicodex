use super::*;
use codex_utils_output_truncation::approx_token_count;

#[test]
fn truncates_custom_mode_hint_to_context_item_limit() {
    let instructions = MultiAgentModeInstructions::from_mode(MultiAgentMode::Custom(
        "large mode hint ".repeat(20_000),
    ))
    .expect("non-empty custom hint should produce instructions");

    assert!(approx_token_count(&instructions.body()) <= MAX_MULTI_AGENT_MODE_HINT_TOKENS);
}
