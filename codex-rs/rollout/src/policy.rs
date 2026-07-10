use crate::protocol::EventMsg;
use crate::protocol::RolloutItem;
use codex_protocol::items::TurnItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_utils_string::truncate_middle_chars;

const PERSISTED_EXEC_AGGREGATED_OUTPUT_MAX_BYTES: usize = 10_000;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum EventPersistenceMode {
    #[default]
    Limited,
    Extended,
}

/// Whether a rollout `item` should be persisted in rollout files.
pub fn is_persisted_rollout_item(item: &RolloutItem, history_mode: ThreadHistoryMode) -> bool {
    is_persisted_rollout_item_with_mode(item, history_mode, EventPersistenceMode::Limited)
}

/// Whether a rollout `item` should be persisted for the thread history format and event detail
/// level.
pub fn is_persisted_rollout_item_with_mode(
    item: &RolloutItem,
    history_mode: ThreadHistoryMode,
    event_mode: EventPersistenceMode,
) -> bool {
    match item {
        RolloutItem::ResponseItem(item) => should_persist_response_item(item),
        RolloutItem::InterAgentCommunication(_)
        | RolloutItem::InterAgentCommunicationMetadata { .. } => true,
        RolloutItem::EventMsg(ev) => {
            should_persist_event_msg_with_mode(ev, history_mode, event_mode)
        }
        // Persist Codex executive markers so we can analyze flows (e.g., compaction, API turns).
        RolloutItem::Compacted(_)
        | RolloutItem::TurnContext(_)
        | RolloutItem::WorldState(_)
        | RolloutItem::SessionMeta(_) => true,
    }
}

/// Return the rollout items that should be persisted for a live append.
pub fn persisted_rollout_items(
    items: &[RolloutItem],
    history_mode: ThreadHistoryMode,
) -> Vec<RolloutItem> {
    persisted_rollout_items_with_mode(items, history_mode, EventPersistenceMode::Limited)
}

/// Return rollout items filtered by both the thread history format and event detail level.
pub fn persisted_rollout_items_with_mode(
    items: &[RolloutItem],
    history_mode: ThreadHistoryMode,
    event_mode: EventPersistenceMode,
) -> Vec<RolloutItem> {
    let mut persisted = Vec::new();
    for item in items {
        if is_persisted_rollout_item_with_mode(item, history_mode, event_mode) {
            persisted.push(sanitize_rollout_item_for_persistence(
                item.clone(),
                event_mode,
            ));
        }
    }
    persisted
}

pub(crate) fn sanitize_rollout_item_for_persistence(
    item: RolloutItem,
    mode: EventPersistenceMode,
) -> RolloutItem {
    if mode != EventPersistenceMode::Extended {
        return item;
    }

    match item {
        RolloutItem::EventMsg(EventMsg::ExecCommandEnd(mut event)) => {
            event.aggregated_output = truncate_middle_chars(
                &event.aggregated_output,
                PERSISTED_EXEC_AGGREGATED_OUTPUT_MAX_BYTES,
            );
            event.stdout.clear();
            event.stderr.clear();
            event.formatted_output.clear();
            RolloutItem::EventMsg(EventMsg::ExecCommandEnd(event))
        }
        _ => item,
    }
}

/// Whether a `ResponseItem` should be persisted in rollout files.
#[inline]
pub fn should_persist_response_item(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message { .. }
        | ResponseItem::AgentMessage { .. }
        | ResponseItem::Reasoning { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::FunctionCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::WebSearchCall { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::ContextCompaction { .. } => true,
        ResponseItem::AdditionalTools { .. }
        | ResponseItem::CompactionTrigger { .. }
        | ResponseItem::Other => false,
    }
}

/// Whether a `ResponseItem` should be persisted for the memories.
#[inline]
pub fn should_persist_response_item_for_memories(item: &ResponseItem) -> bool {
    match item {
        ResponseItem::Message { role, .. } => role != "developer",
        ResponseItem::AgentMessage { .. }
        | ResponseItem::LocalShellCall { .. }
        | ResponseItem::FunctionCall { .. }
        | ResponseItem::ToolSearchCall { .. }
        | ResponseItem::FunctionCallOutput { .. }
        | ResponseItem::ToolSearchOutput { .. }
        | ResponseItem::CustomToolCall { .. }
        | ResponseItem::CustomToolCallOutput { .. }
        | ResponseItem::WebSearchCall { .. } => true,
        ResponseItem::AdditionalTools { .. }
        | ResponseItem::Reasoning { .. }
        | ResponseItem::ImageGenerationCall { .. }
        | ResponseItem::Compaction { .. }
        | ResponseItem::CompactionTrigger { .. }
        | ResponseItem::ContextCompaction { .. }
        | ResponseItem::Other => false,
    }
}

/// Whether an `EventMsg` should be persisted for the thread history format and event detail level.
#[inline]
pub fn should_persist_event_msg_with_mode(
    ev: &EventMsg,
    history_mode: ThreadHistoryMode,
    event_mode: EventPersistenceMode,
) -> bool {
    match ev {
        EventMsg::ItemCompleted(event) => {
            // Paginated rollouts store TurnItems.
            // Legacy rollouts keep only items with no raw ResponseItem or legacy equivalent.
            matches!(history_mode, ThreadHistoryMode::Paginated)
                || matches!(event.item, TurnItem::Plan(_) | TurnItem::Sleep(_))
        }
        EventMsg::TokenCount(_)
        | EventMsg::ThreadGoalUpdated(_)
        | EventMsg::ThreadRolledBack(_)
        | EventMsg::TurnAborted(_)
        | EventMsg::TurnStarted(_)
        | EventMsg::TurnComplete(_)
        | EventMsg::ThreadSettingsApplied(_) => true,

        // Only persist these legacy events when the thread's history mode is Legacy.
        // New, paginated rollouts persist ItemCompleted events with TurnItems.
        EventMsg::UserMessage(_)
        | EventMsg::AgentMessage(_)
        | EventMsg::AgentReasoning(_)
        | EventMsg::AgentReasoningRawContent(_)
        | EventMsg::EnteredReviewMode(_)
        | EventMsg::ExitedReviewMode(_)
        | EventMsg::PatchApplyEnd(_)
        | EventMsg::ContextCompacted(_)
        | EventMsg::McpToolCallEnd(_)
        | EventMsg::WebSearchEnd(_)
        | EventMsg::ImageGenerationEnd(_)
        | EventMsg::SubAgentActivity(_) => matches!(history_mode, ThreadHistoryMode::Legacy),

        // Extended history keeps terminal details that are useful for full-fidelity replay but
        // are too verbose for the default rollout representation.
        EventMsg::Error(_)
        | EventMsg::GuardianAssessment(_)
        | EventMsg::ExecCommandEnd(_)
        | EventMsg::ViewImageToolCall(_)
        | EventMsg::CollabAgentSpawnEnd(_)
        | EventMsg::CollabAgentInteractionEnd(_)
        | EventMsg::CollabWaitingEnd(_)
        | EventMsg::CollabCloseEnd(_)
        | EventMsg::CollabResumeEnd(_)
        | EventMsg::DynamicToolCallRequest(_)
        | EventMsg::DynamicToolCallResponse(_) => {
            matches!(event_mode, EventPersistenceMode::Extended)
        }

        // Transient, non-durable events.
        EventMsg::Warning(_)
        | EventMsg::GuardianWarning(_)
        | EventMsg::RealtimeConversationStarted(_)
        | EventMsg::RealtimeConversationSdp(_)
        | EventMsg::RealtimeConversationRealtime(_)
        | EventMsg::RealtimeConversationClosed(_)
        | EventMsg::SafetyBuffering(_)
        | EventMsg::ModelReroute(_)
        | EventMsg::ModelVerification(_)
        | EventMsg::TurnModerationMetadata(_)
        | EventMsg::AgentReasoningSectionBreak(_)
        | EventMsg::RawResponseItem(_)
        | EventMsg::SessionConfigured(_)
        | EventMsg::McpToolCallBegin(_)
        | EventMsg::ExecCommandBegin(_)
        | EventMsg::TerminalInteraction(_)
        | EventMsg::ExecCommandOutputDelta(_)
        | EventMsg::ExecApprovalRequest(_)
        | EventMsg::RequestPermissions(_)
        | EventMsg::RequestUserInput(_)
        | EventMsg::ElicitationRequest(_)
        | EventMsg::ApplyPatchApprovalRequest(_)
        | EventMsg::StreamError(_)
        | EventMsg::PatchApplyBegin(_)
        | EventMsg::PatchApplyUpdated(_)
        | EventMsg::TurnDiff(_)
        | EventMsg::RealtimeConversationListVoicesResponse(_)
        | EventMsg::McpStartupUpdate(_)
        | EventMsg::McpStartupComplete(_)
        | EventMsg::WebSearchBegin(_)
        | EventMsg::PlanUpdate(_)
        | EventMsg::ShutdownComplete
        | EventMsg::DeprecationNotice(_)
        | EventMsg::ItemStarted(_)
        | EventMsg::HookStarted(_)
        | EventMsg::HookCompleted(_)
        | EventMsg::AgentMessageContentDelta(_)
        | EventMsg::PlanDelta(_)
        | EventMsg::ReasoningContentDelta(_)
        | EventMsg::ReasoningRawContentDelta(_)
        | EventMsg::ImageGenerationBegin(_)
        | EventMsg::CollabAgentSpawnBegin(_)
        | EventMsg::CollabAgentInteractionBegin(_)
        | EventMsg::CollabWaitingBegin(_)
        | EventMsg::CollabCloseBegin(_)
        | EventMsg::CollabResumeBegin(_) => false,
    }
}
