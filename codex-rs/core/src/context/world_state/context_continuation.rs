use super::PreviousSectionState;
use super::WorldStateSection;
use crate::context::ContextualUserFragment;
use codex_protocol::models::ContentItemKind;

const INSTRUCTIONS: &str = "The runtime manages context compaction or rollover. \
Context-window capacity is not a task deadline or the goal's token budget. \
Cumulative token usage across requests is not the current context-window size. \
Continue the authorized work and tool calls while work remains; do not stop, defer it to a new conversation, \
or claim this turn's context is exhausted based on your own estimate. \
After compaction, use the retained summary and current state to continue the same task. \
Do not mark an unfinished goal complete or blocked because the context is filling up or a response is ending. \
Actual runtime errors, user stop requests, permissions, and explicit goal or usage limits still apply; \
report real failures accurately instead of claiming success.";

/// Runtime continuation policy, independent of optional model-owned window-management guidance.
#[derive(Clone, Debug)]
pub(crate) struct ContextContinuationState;

impl ContextualUserFragment for ContextContinuationState {
    fn content_kind(&self) -> ContentItemKind {
        ContentItemKind("context.continuation".to_string())
    }

    fn role(&self) -> &'static str {
        "developer"
    }

    fn markers(&self) -> (&'static str, &'static str) {
        Self::type_markers()
    }

    fn type_markers() -> (&'static str, &'static str) {
        ("<context_continuation>", "</context_continuation>")
    }

    fn body(&self) -> String {
        format!("\n{INSTRUCTIONS}\n")
    }
}

impl WorldStateSection for ContextContinuationState {
    const ID: &'static str = "context_continuation";
    type Snapshot = String;

    fn snapshot(&self) -> Self::Snapshot {
        INSTRUCTIONS.to_string()
    }

    fn matches_legacy_fragment(role: &str, text: &str) -> bool {
        role == "developer" && Self::matches_text(text)
    }

    fn has_retained_fragment_matcher() -> bool {
        true
    }

    fn matches_retained_fragment(role: &str, text: &str) -> bool {
        Self::matches_legacy_fragment(role, text)
    }

    fn render_diff(
        &self,
        previous: PreviousSectionState<'_, Self::Snapshot>,
    ) -> Option<Box<dyn ContextualUserFragment>> {
        if matches!(previous, PreviousSectionState::Known(message) if message == INSTRUCTIONS) {
            return None;
        }
        Some(Box::new(self.clone()))
    }
}
