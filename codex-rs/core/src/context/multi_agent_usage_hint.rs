use super::ContextualUserFragment;
use codex_utils_output_truncation::TruncationPolicy;
use codex_utils_output_truncation::truncate_text;

const MAX_MULTI_AGENT_USAGE_HINT_TOKENS: usize = 8_000;
const TRUNCATION_MARKER_TOKEN_RESERVE: usize = 128;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultiAgentUsageHint {
    text: String,
}

impl MultiAgentUsageHint {
    pub(crate) fn new(text: &str) -> Self {
        Self {
            text: truncate_text(
                text,
                TruncationPolicy::Tokens(
                    MAX_MULTI_AGENT_USAGE_HINT_TOKENS - TRUNCATION_MARKER_TOKEN_RESERVE,
                ),
            ),
        }
    }
}

impl ContextualUserFragment for MultiAgentUsageHint {
    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("", "")
    }

    fn body(&self) -> String {
        self.text.clone()
    }
}

#[cfg(test)]
#[path = "multi_agent_usage_hint_tests.rs"]
mod tests;
