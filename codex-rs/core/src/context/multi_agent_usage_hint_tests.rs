use super::*;
use codex_utils_output_truncation::approx_token_count;

#[test]
fn usage_hint_is_bounded_at_the_fragment_boundary() {
    let hint = MultiAgentUsageHint::new(&"multi-agent usage hint ".repeat(2_000));

    assert!(approx_token_count(&hint.body()) <= MULTI_AGENT_USAGE_HINT_MAX_TOKENS);
}
