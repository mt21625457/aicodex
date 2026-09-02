use super::ContextualUserFragment;
use crate::config::MULTI_AGENT_USAGE_HINT_MAX_TOKENS;
use crate::config::truncate_text_to_token_budget;
use codex_protocol::models::ContentItemKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MultiAgentRoleInstructions {
    text: String,
    marked: bool,
}

impl MultiAgentRoleInstructions {
    pub(crate) fn unmarked(text: impl Into<String>) -> Self {
        Self {
            text: truncate_text_to_token_budget(&text.into(), MULTI_AGENT_USAGE_HINT_MAX_TOKENS),
            marked: false,
        }
    }

    pub(crate) fn catalog(text: impl Into<String>) -> Self {
        Self {
            text: truncate_text_to_token_budget(&text.into(), MULTI_AGENT_USAGE_HINT_MAX_TOKENS),
            marked: true,
        }
    }
}

impl ContextualUserFragment for MultiAgentRoleInstructions {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("multi_agent.role_instructions".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn requires_separate_message(&self) -> bool {
        true
    }

    fn markers(&self) -> (&'static str, &'static str) {
        if self.marked {
            Self::type_markers()
        } else {
            ("", "")
        }
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<multi_agent_role>", "</multi_agent_role>")
    }

    fn body(&self) -> String {
        self.text.clone()
    }
}
