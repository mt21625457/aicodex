use super::ContextualUserFragment;
use crate::config::MULTI_AGENT_USAGE_HINT_MAX_TOKENS;
use crate::config::truncate_text_to_token_budget;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ContentItemKind;
use codex_protocol::models::input_text;

#[derive(Debug, Clone, PartialEq, Eq)]
/// Configured multi-agent instructions emitted as a standalone developer message.
pub(crate) struct MultiAgentUsageHint {
    text: String,
    marked: bool,
}

impl MultiAgentUsageHint {
    pub(crate) const KIND: &str = "multi_agent.usage_hint";

    /// Bound the fully composed role text, including runtime guidance, at the context boundary.
    pub(crate) fn from_role(instructions: &super::MultiAgentRoleInstructions) -> Self {
        Self {
            text: truncate_text_to_token_budget(
                &instructions.body().into_text().unwrap_or_default(),
                MULTI_AGENT_USAGE_HINT_MAX_TOKENS,
            ),
            marked: !instructions.markers().0.is_empty(),
        }
    }
}

impl ContextualUserFragment for MultiAgentUsageHint {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind(
            if self.marked {
                "multi_agent.role_instructions"
            } else {
                Self::KIND
            }
            .to_string(),
        )
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        if self.marked {
            ("<multi_agent_role>", "</multi_agent_role>")
        } else {
            Self::type_markers()
        }
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("", "")
    }

    fn body(&self) -> ContentItem {
        input_text(self.text.clone())
    }
}
#[cfg(test)]
#[path = "multi_agent_usage_hint_tests.rs"]
mod tests;
