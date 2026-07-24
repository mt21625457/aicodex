use super::*;
use codex_utils_output_truncation::approx_token_count;

#[test]
fn truncates_usage_hint_to_context_item_limit() {
    let hint = MultiAgentUsageHint::new(&"large hint ".repeat(20_000));

    assert!(approx_token_count(&hint.body()) <= MAX_MULTI_AGENT_USAGE_HINT_TOKENS);
}
