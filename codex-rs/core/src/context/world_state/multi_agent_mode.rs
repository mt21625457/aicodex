use super::PreviousSectionState;
use super::WorldStateSection;
use crate::config::MULTI_AGENT_MODE_MAX_TOKENS;
use crate::config::truncate_text_to_token_budget;
use crate::context::ContextualUserFragment;
use crate::context::multi_agent_mode_instructions::MultiAgentModeInstructions;
use codex_protocol::config_types::MultiAgentMode;
use serde::Deserialize;
use serde::Serialize;

/// Effective multi-agent mode currently visible to the model.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize)]
pub(crate) struct MultiAgentModeState {
    mode: Option<MultiAgentMode>,
}

impl MultiAgentModeState {
    pub(crate) fn new(mode: Option<MultiAgentMode>) -> Self {
        Self {
            mode: mode.map(|mode| match mode {
                MultiAgentMode::Custom(hint_text) => MultiAgentMode::Custom(
                    truncate_text_to_token_budget(&hint_text, MULTI_AGENT_MODE_MAX_TOKENS),
                ),
                mode @ (MultiAgentMode::ExplicitRequestOnly | MultiAgentMode::Proactive) => mode,
            }),
        }
    }
}

impl WorldStateSection for MultiAgentModeState {
    const ID: &'static str = "multi_agent_mode";
    type Snapshot = Self;

    fn snapshot(&self) -> Self::Snapshot {
        self.clone()
    }

    fn matches_legacy_fragment(role: &str, text: &str) -> bool {
        role == "developer" && MultiAgentModeInstructions::matches_text(text)
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
        let current_mode = rendered_mode(self.mode.as_ref());
        let mode = match (current_mode, previous) {
            (Some(mode), PreviousSectionState::Known(previous))
                if rendered_mode(previous.mode.as_ref()) == Some(mode) =>
            {
                return None;
            }
            (Some(mode), _) => mode.clone(),
            (None, PreviousSectionState::Known(previous))
                if matches!(
                    rendered_mode(previous.mode.as_ref()),
                    Some(MultiAgentMode::Proactive | MultiAgentMode::Custom(_))
                ) =>
            {
                MultiAgentMode::ExplicitRequestOnly
            }
            (None, PreviousSectionState::Unknown) => MultiAgentMode::ExplicitRequestOnly,
            (None, PreviousSectionState::Absent | PreviousSectionState::Known(_)) => return None,
        };

        MultiAgentModeInstructions::from_mode(mode)
            .map(|instructions| Box::new(instructions) as Box<dyn ContextualUserFragment>)
    }
}

fn rendered_mode(mode: Option<&MultiAgentMode>) -> Option<&MultiAgentMode> {
    mode.filter(|mode| !matches!(mode, MultiAgentMode::Custom(hint_text) if hint_text.is_empty()))
}

#[cfg(test)]
#[path = "multi_agent_mode_tests.rs"]
mod tests;
