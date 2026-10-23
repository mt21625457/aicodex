//! Replays persisted token usage snapshots when a client attaches to an existing thread.
//!
//! The message processor decides when replay is allowed and preserves JSON-RPC response
//! ordering. This module owns notification construction and the attribution rules that
//! map the latest persisted `TokenCount` back to a v2 turn id.
//!
//! Rollout histories can contain explicit turn ids or generated turn ids. When explicit
//! ids do not match the rebuilt thread, replay falls back to the active turn position at
//! the time the `TokenCount` was persisted so the notification still targets the
//! corresponding rebuilt turn.

use std::sync::Arc;

use codex_app_server_protocol::ServerNotification;
use codex_app_server_protocol::ThreadHistoryBuilder;
use codex_app_server_protocol::ThreadTokenUsage;
use codex_app_server_protocol::ThreadTokenUsageUpdatedNotification;
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::TurnStatus;
use codex_core::CodexThread;
use codex_protocol::ThreadId;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::TokenUsageInfo;
use codex_rollout::RolloutItem;

use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::OutgoingMessageSender;

/// Sends a restored token usage update to the connection that attached to a thread.
///
/// This is lifecycle replay rather than a model event: the rollout already contains
/// the original `TokenCount`, and emitting through `send_event` here would duplicate
/// persisted usage records. Keeping replay connection-scoped also avoids
/// surprising other subscribers with a historical usage update while they may be
/// rendering live turn events.
pub(super) async fn send_thread_token_usage_update_to_connection(
    outgoing: &Arc<OutgoingMessageSender>,
    connection_id: ConnectionId,
    thread_id: ThreadId,
    conversation: &CodexThread,
    token_usage_turn_id: String,
) {
    let Some(info) = conversation.token_usage_info().await else {
        return;
    };
    send_thread_token_usage_update(
        outgoing,
        connection_id,
        thread_id,
        info,
        token_usage_turn_id,
    )
    .await;
}

/// Sends a token usage snapshot that was restored directly from persisted history.
pub(super) async fn send_thread_token_usage_info_update_to_connection(
    outgoing: &Arc<OutgoingMessageSender>,
    connection_id: ConnectionId,
    thread_id: ThreadId,
    info: TokenUsageInfo,
    token_usage_turn_id: String,
) {
    send_thread_token_usage_update(
        outgoing,
        connection_id,
        thread_id,
        info,
        token_usage_turn_id,
    )
    .await;
}

async fn send_thread_token_usage_update(
    outgoing: &Arc<OutgoingMessageSender>,
    connection_id: ConnectionId,
    thread_id: ThreadId,
    info: TokenUsageInfo,
    token_usage_turn_id: String,
) {
    let notification = ThreadTokenUsageUpdatedNotification {
        thread_id: thread_id.to_string(),
        turn_id: token_usage_turn_id,
        token_usage: ThreadTokenUsage::from(info),
    };
    outgoing
        .send_server_notification_to_connections(
            &[connection_id],
            ServerNotification::ThreadTokenUsageUpdated(notification),
        )
        .await;
}

/// Returns the latest persisted token usage snapshot, ignoring rate-limit-only records.
pub(super) fn latest_token_usage_info_from_rollout_items(
    rollout_items: &[RolloutItem],
) -> Option<TokenUsageInfo> {
    rollout_items.iter().rev().find_map(|item| match item {
        RolloutItem::EventMsg(EventMsg::TokenCount(event)) => event.info.clone(),
        _ => None,
    })
}

pub(super) fn restored_token_usage_turn_id(
    rollout_items: &[RolloutItem],
    turns: &[Turn],
) -> String {
    latest_token_usage_turn_id_from_rollout_items(rollout_items, turns)
        .unwrap_or_else(|| latest_token_usage_turn_id(turns))
}

/// Identifies the turn that was active when the latest `TokenCount` record appeared.
///
/// The id is preferred when it still appears in the rebuilt thread. The position is a
/// fallback for histories whose implicit turn ids are regenerated during reconstruction.
struct TokenUsageTurnOwner {
    id: String,
    position: Option<usize>,
    has_explicit_id: bool,
}

pub(super) fn latest_token_usage_turn_id_from_rollout_items(
    rollout_items: &[RolloutItem],
    turns: &[Turn],
) -> Option<String> {
    let mut builder = ThreadHistoryBuilder::new();
    let mut token_usage_turn_owner = None;
    let mut explicit_turn_id = None;

    // 最后一个有效 usage 之后的事件不会再改变已捕获的归属；返回值仍使用调用方
    // 提供的完整 turns 进行显式 id / position 回退，因此无需重放尾部的大正文。
    let last_usage_index = rollout_items.iter().rposition(|item| {
        matches!(
            item, RolloutItem::EventMsg(EventMsg::TokenCount(event)) if event.info.is_some()
        )
    })?;
    for item in &rollout_items[..=last_usage_index] {
        if let RolloutItem::EventMsg(EventMsg::TurnStarted(event)) = item {
            explicit_turn_id = Some(event.turn_id.as_str());
        }
        if matches!(
            item,
            RolloutItem::EventMsg(EventMsg::TokenCount(event)) if event.info.is_some()
        ) {
            // 归属判断只读取 id 和位置；借用访问器保留完整快照的最后完成 turn 回退，
            // 避免每条 TokenCount 都复制此前累计的正文。事件重放顺序保持不变。
            token_usage_turn_owner = builder.active_turn_id().map(|turn_id| TokenUsageTurnOwner {
                has_explicit_id: explicit_turn_id == Some(turn_id),
                id: turn_id.to_owned(),
                position: builder.active_turn_position(),
            });
        }
        builder.handle_rollout_item(item);
    }

    let owner = token_usage_turn_owner?;
    if owner.has_explicit_id || turns.iter().any(|turn| turn.id == owner.id) {
        Some(owner.id)
    } else {
        owner
            .position
            .and_then(|position| turns.get(position))
            .map(|turn| turn.id.clone())
    }
}

/// 在 Thread 移入响应之前解析通知归属；Some("") 也是明确给定的 id，不能触发回退。
pub(super) fn resolve_token_usage_turn_id(turns: &[Turn], turn_id: Option<String>) -> String {
    turn_id.unwrap_or_else(|| latest_token_usage_turn_id(turns))
}

/// Chooses a fallback turn id that should own a replayed token usage update.
///
/// Normal replay derives the owner from the rollout position of the latest
/// `TokenCount` event. This fallback only preserves a stable wire shape for
/// unusual histories where that rollout information cannot be read.
fn latest_token_usage_turn_id(turns: &[Turn]) -> String {
    turns
        .iter()
        .rev()
        .find(|turn| matches!(turn.status, TurnStatus::Completed | TurnStatus::Failed))
        .or_else(|| turns.last())
        .map(|turn| turn.id.clone())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::build_turns_from_rollout_items;
    use codex_protocol::protocol::AgentMessageEvent;
    use codex_protocol::protocol::TokenCountEvent;
    use codex_protocol::protocol::UserMessageEvent;
    use pretty_assertions::assert_eq;

    #[test]
    fn replay_attribution_uses_already_loaded_history() {
        let rollout_items = token_usage_history();
        let turns = build_turns_from_rollout_items(&rollout_items);

        assert_eq!(
            latest_token_usage_turn_id_from_rollout_items(&rollout_items, turns.as_slice()),
            Some(turns[0].id.clone())
        );
    }

    #[test]
    fn replay_attribution_falls_back_to_rebuilt_turn_position() {
        let rollout_items = token_usage_history();
        let mut turns = build_turns_from_rollout_items(&rollout_items);
        turns[0].id = "rebuilt-turn-id".to_string();

        assert_eq!(
            latest_token_usage_turn_id_from_rollout_items(&rollout_items, turns.as_slice()),
            Some("rebuilt-turn-id".to_string())
        );
    }

    #[test]
    fn replay_usage_ignores_newer_rate_limit_only_records() {
        let mut rollout_items = token_usage_history();
        let expected = TokenUsageInfo::empty(None);
        rollout_items.push(RolloutItem::EventMsg(EventMsg::TokenCount(
            TokenCountEvent {
                info: None,
                rate_limits: None,
            },
        )));

        assert_eq!(
            latest_token_usage_info_from_rollout_items(&rollout_items),
            Some(expected)
        );

        let turns = build_turns_from_rollout_items(&rollout_items);
        assert_eq!(
            latest_token_usage_turn_id_from_rollout_items(&rollout_items, &turns),
            Some(turns[0].id.clone())
        );

        assert_eq!(
            latest_token_usage_turn_id_from_rollout_items(&rollout_items, /*turns*/ &[]),
            None
        );
    }

    #[test]
    fn replay_attribution_rejects_suffix_generated_turn_ids() {
        let rollout_items = token_usage_history();

        assert_eq!(
            latest_token_usage_turn_id_from_rollout_items(&rollout_items, /*turns*/ &[]),
            None
        );
    }

    #[test]
    fn replay_attribution_uses_explicit_turn_id_without_loaded_turns() {
        let rollout_items = vec![
            RolloutItem::EventMsg(EventMsg::TurnStarted(
                codex_protocol::protocol::TurnStartedEvent {
                    turn_attribution: None,
                    root_turn_id: None,
                    turn_id: "persisted-turn-id".to_string(),
                    trace_id: None,
                    started_at: None,
                    model_context_window: None,
                    collaboration_mode_kind: Default::default(),
                },
            )),
            RolloutItem::EventMsg(EventMsg::TokenCount(TokenCountEvent {
                info: Some(TokenUsageInfo::empty(None)),
                rate_limits: None,
            })),
        ];

        assert_eq!(
            latest_token_usage_turn_id_from_rollout_items(&rollout_items, /*turns*/ &[]),
            Some("persisted-turn-id".to_string())
        );
    }

    #[test]
    fn replay_attribution_uses_latest_token_count_and_ignores_tail_turn() {
        let mut rollout_items = token_usage_history();
        rollout_items.extend(token_usage_history());
        let turns = build_turns_from_rollout_items(&rollout_items);

        assert_eq!(
            latest_token_usage_turn_id_from_rollout_items(&rollout_items, turns.as_slice()),
            Some(turns[2].id.clone())
        );
    }

    #[test]
    fn replay_notification_turn_id_preserves_explicit_and_fallback_ids() {
        let mut turns = build_turns_from_rollout_items(&token_usage_history());
        turns[0].status = TurnStatus::Completed;
        turns[1].status = TurnStatus::InProgress;
        assert_eq!(resolve_token_usage_turn_id(&turns, None), turns[0].id);
        turns[0].status = TurnStatus::Failed;
        assert_eq!(resolve_token_usage_turn_id(&turns, None), turns[0].id);
        turns[0].status = TurnStatus::Interrupted;
        assert_eq!(resolve_token_usage_turn_id(&turns, None), turns[1].id);
        assert_eq!(resolve_token_usage_turn_id(&turns, Some(String::new())), "");
        assert_eq!(
            resolve_token_usage_turn_id(&[], Some("explicit".to_string())),
            "explicit"
        );
        assert_eq!(resolve_token_usage_turn_id(&[], None), "");
    }

    // 保留旧的完整快照实现作为差分参照，避免测试只重复新访问器的实现细节。
    fn snapshot_replay_reference(items: &[RolloutItem], turns: &[Turn]) -> Option<String> {
        let mut builder = ThreadHistoryBuilder::new();
        let mut owner = None;
        let mut explicit_turn_id = None;
        for item in items {
            if let RolloutItem::EventMsg(EventMsg::TurnStarted(event)) = item {
                explicit_turn_id = Some(event.turn_id.as_str());
            }
            if matches!(item, RolloutItem::EventMsg(EventMsg::TokenCount(event)) if event.info.is_some())
            {
                owner = builder
                    .active_turn_snapshot()
                    .map(|turn| TokenUsageTurnOwner {
                        has_explicit_id: explicit_turn_id == Some(turn.id.as_str()),
                        id: turn.id,
                        position: builder.active_turn_position(),
                    });
            }
            builder.handle_rollout_item(item);
        }
        let owner = owner?;
        if owner.has_explicit_id || turns.iter().any(|turn| turn.id == owner.id) {
            Some(owner.id)
        } else {
            owner
                .position
                .and_then(|position| turns.get(position))
                .map(|turn| turn.id.clone())
        }
    }

    #[test]
    fn replay_attribution_matches_full_snapshots_at_every_history_prefix() {
        use codex_protocol::protocol::ThreadRolledBackEvent;
        use codex_protocol::protocol::TurnAbortReason;
        use codex_protocol::protocol::TurnAbortedEvent;
        use codex_protocol::protocol::TurnStartedEvent;

        let usage = || {
            RolloutItem::EventMsg(EventMsg::TokenCount(TokenCountEvent {
                info: Some(TokenUsageInfo::empty(None)),
                rate_limits: None,
            }))
        };
        let mut items = vec![usage()];
        items.extend(token_usage_history());
        items.push(RolloutItem::EventMsg(EventMsg::TurnStarted(
            TurnStartedEvent {
                turn_attribution: None,
                root_turn_id: None,
                turn_id: "explicit-turn".to_string(),
                trace_id: None,
                started_at: Some(42),
                model_context_window: None,
                collaboration_mode_kind: Default::default(),
            },
        )));
        items.push(RolloutItem::EventMsg(EventMsg::AgentMessage(
            AgentMessageEvent {
                message: "large answer 汉字\n".repeat(8192),
                phase: None,
                memory_citation: None,
                delivery: None,
                questions: None,
            },
        )));
        items.push(usage());
        items.push(RolloutItem::EventMsg(EventMsg::TurnAborted(
            TurnAbortedEvent {
                root_turn_id: None,
                turn_id: Some("explicit-turn".to_string()),
                reason: TurnAbortReason::Replaced,
                error: None,
                started_at: None,
                completed_at: Some(43),
                duration_ms: Some(1000),
            },
        )));
        items.push(usage());
        items.push(RolloutItem::EventMsg(EventMsg::ThreadRolledBack(
            ThreadRolledBackEvent { num_turns: 1 },
        )));
        items.push(usage());
        items.push(RolloutItem::Compacted(Box::new(
            codex_rollout::CompactedItem {
                message: "summary".to_string(),
                replacement_history: None,
                retained_context: None,
                guardian_history: None,
                mcp_resource_origins: None,
                window_number: None,
                first_window_id: None,
                previous_window_id: None,
                window_id: None,
                compaction_response_id: None,
                latest_token_usage_record: None,
                resume_metadata: None,
            },
        )));
        items.push(usage());
        items.push(RolloutItem::EventMsg(EventMsg::TokenCount(
            TokenCountEvent {
                info: None,
                rate_limits: None,
            },
        )));
        for end in 0..=items.len() {
            let prefix = &items[..end];
            let turns = build_turns_from_rollout_items(prefix);
            let mut remapped = turns.clone();
            for (index, turn) in remapped.iter_mut().enumerate() {
                turn.id = format!("rebuilt-{index}");
            }
            for projected in [&turns[..], &remapped[..], &[]] {
                assert_eq!(
                    latest_token_usage_turn_id_from_rollout_items(prefix, projected),
                    snapshot_replay_reference(prefix, projected),
                    "history prefix {end}"
                );
            }
        }
    }

    fn token_usage_history() -> Vec<RolloutItem> {
        vec![
            RolloutItem::EventMsg(EventMsg::UserMessage(UserMessageEvent {
                client_id: None,
                message: "first turn".to_string(),
                images: None,
                local_images: Vec::new(),
                text_elements: Vec::new(),
                ..Default::default()
            })),
            RolloutItem::EventMsg(EventMsg::AgentMessage(AgentMessageEvent {
                message: "first answer".to_string(),
                phase: None,
                memory_citation: None,
                delivery: None,
                questions: None,
            })),
            RolloutItem::EventMsg(EventMsg::TokenCount(TokenCountEvent {
                info: Some(TokenUsageInfo::empty(None)),
                rate_limits: None,
            })),
            RolloutItem::EventMsg(EventMsg::UserMessage(UserMessageEvent {
                client_id: None,
                message: "second turn".to_string(),
                images: None,
                local_images: Vec::new(),
                text_elements: Vec::new(),
                ..Default::default()
            })),
        ]
    }
}
