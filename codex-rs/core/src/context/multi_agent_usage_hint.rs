use super::ContextualUserFragment;
use crate::config::MULTI_AGENT_USAGE_HINT_MAX_TOKENS;
use crate::config::truncate_text_to_token_budget;
use codex_protocol::models::ContentItemKind;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Configured multi-agent instructions emitted as a standalone developer message.
pub(crate) struct MultiAgentUsageHint {
    text: String,
}

impl MultiAgentUsageHint {
    pub(crate) fn new(text: &str) -> Self {
        Self {
            text: truncate_text_to_token_budget(text, MULTI_AGENT_USAGE_HINT_MAX_TOKENS),
        }
    }
}

impl ContextualUserFragment for MultiAgentUsageHint {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("multi_agent.usage_hint".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn requires_separate_message(&self) -> bool {
        true
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
