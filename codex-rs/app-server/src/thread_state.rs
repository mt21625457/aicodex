use crate::outgoing_message::ConnectionId;
use crate::outgoing_message::ConnectionRequestId;
use codex_app_server_protocol::RequestId;
use codex_app_server_protocol::ThreadGoal;
use codex_app_server_protocol::ThreadHistoryBuilder;
use codex_app_server_protocol::ThreadHistoryTurnMetadata;
use codex_app_server_protocol::ThreadItem;
#[cfg(test)]
use codex_app_server_protocol::ThreadItemPayload;
use codex_app_server_protocol::ThreadRuntimeLifecycle;
use codex_app_server_protocol::ThreadSettings;
use codex_app_server_protocol::TranscriptMetadata;
use codex_app_server_protocol::Turn;
use codex_app_server_protocol::TurnError;
use codex_app_server_protocol::TurnItemsView;
use codex_core::CodexThread;
use codex_core::ThreadConfigSnapshot;
use codex_file_watcher::WatchRegistration;
use codex_protocol::ThreadId;
#[cfg(test)]
use codex_protocol::config_types::MultiAgentMode;
use codex_protocol::items::AgentMessageContent as CoreAgentMessageContent;
use codex_protocol::items::TurnItem as CoreTurnItem;
use codex_protocol::models::MessagePhase;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::ThreadHistoryMode;
use codex_rollout::RolloutItem;
use codex_rollout::state_db::StateDbHandle;
use codex_utils_path_uri::LegacyAppPathString;
use std::collections::HashMap;
use std::collections::HashSet;
use std::collections::hash_map::Entry;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::Weak;
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::oneshot;
use tokio::sync::watch;
use tracing::error;

type PendingInterruptQueue = Vec<ConnectionRequestId>;

// The unload that inserts an entry owns its removal. Dropping the sender wakes deferred
// listener attachment only after that unload finishes cleanup.
pub(crate) type PendingThreadUnloads = Arc<Mutex<HashMap<ThreadId, watch::Sender<()>>>>;

pub(crate) struct PendingThreadResumeRequest {
    pub(crate) request_id: ConnectionRequestId,
    pub(crate) history_items: Vec<RolloutItem>,
    /// Usage attribution already resolved while cold-loading a paginated child.
    pub(crate) cold_resume_token_usage_turn_id: Option<String>,
    pub(crate) config_snapshot: ThreadConfigSnapshot,
    pub(crate) instruction_sources: Vec<LegacyAppPathString>,
    pub(crate) thread_summary: codex_app_server_protocol::Thread,
    pub(crate) emit_thread_goal_update: bool,
    pub(crate) thread_goal_state_db: Option<StateDbHandle>,
    pub(crate) include_turns: bool,
    pub(crate) initial_turns_page:
        Option<codex_app_server_protocol::ThreadResumeInitialTurnsPageParams>,
    pub(crate) paginated_turns: Option<Vec<Turn>>,
    pub(crate) paginated_initial_turns_page: Option<codex_app_server_protocol::TurnsPage>,
    pub(crate) paginated_initial_turns_page_with_active_slot:
        Option<codex_app_server_protocol::TurnsPage>,
    pub(crate) resume_cursor_store: Option<Arc<dyn codex_thread_store::ThreadStore>>,
    pub(crate) redact_resume_payloads: bool,
}

// ThreadListenerCommand is used to perform operations in the context of the thread listener, for serialization purposes.
pub(crate) enum ThreadListenerCommand {
    // SendThreadResumeResponse is used to resume an already running thread by sending the thread's history to the client and atomically subscribing for new updates.
    SendThreadResumeResponse {
        request: Box<PendingThreadResumeRequest>,
        completion_tx: oneshot::Sender<()>,
    },
    // EmitThreadGoalUpdated is used to order goal updates with running-thread resume responses and goal clears.
    EmitThreadGoalUpdated {
        turn_id: Option<String>,
        goal: ThreadGoal,
    },
    // EmitThreadQueueChanged orders durable queue updates with thread notifications.
    EmitThreadQueueChanged,
    // EmitWarning is used to order extension warnings with other thread notifications.
    EmitWarning {
        message: String,
    },
    // EmitThreadGoalCleared is used to order app-server goal clears with running-thread resume responses.
    EmitThreadGoalCleared,
    // EmitThreadGoalSnapshot is used to read and emit the latest goal state in the listener order.
    EmitThreadGoalSnapshot {
        state_db: StateDbHandle,
    },
    // ResolveServerRequest is used to notify the client that the request has been resolved.
    // It is executed in the thread listener's context to ensure that the resolved notification is ordered with regard to the request itself.
    ResolveServerRequest {
        request_id: RequestId,
        completion_tx: oneshot::Sender<()>,
    },
}

/// Retained until core resolves or withdraws the prompt, even if the client replies first.
pub(crate) struct PendingNetworkApproval {
    pub(crate) request_id: RequestId,
    pub(crate) permission_guard: crate::thread_status::ThreadWatchActiveGuard,
}

/// Per-conversation accumulation of the latest states e.g. error message while a turn runs.
#[derive(Default, Clone)]
pub(crate) struct TurnSummary {
    pub(crate) started_at: Option<i64>,
    pub(crate) command_start_times: HashMap<String, i64>,
    pub(crate) last_error: Option<TurnError>,
    pub(crate) last_agent_message: Option<ThreadItem>,
}

impl TurnSummary {
    pub(crate) fn record_command_start(&mut self, item_id: String, started_at_ms: i64) -> bool {
        if let std::collections::hash_map::Entry::Vacant(entry) =
            self.command_start_times.entry(item_id)
        {
            entry.insert(started_at_ms);
            true
        } else {
            false
        }
    }
}

#[derive(Default)]
pub(crate) struct ThreadState {
    goal_resume_lock: Arc<Mutex<()>>,
    pub(crate) pending_interrupts: PendingInterruptQueue,
    pub(crate) network_approvals: HashMap<String, PendingNetworkApproval>,
    pub(crate) turn_summary: TurnSummary,
    pub(crate) declined_command_executions: HashSet<(String, String)>,
    pub(crate) last_terminal_turn_id: Option<String>,
    /// Lets an internal runtime replacement wait until the old listener has processed Core's
    /// `ShutdownComplete` event before that listener is superseded.
    pub(crate) shutdown_drain: Option<watch::Receiver<bool>>,
    pub(crate) cancel_tx: Option<oneshot::Sender<()>>,
    pub(crate) experimental_raw_events: bool,
    pub(crate) listener_generation: u64,
    last_thread_settings: Option<ThreadSettings>,
    listener_command_tx: Option<mpsc::UnboundedSender<ThreadListenerCommand>>,
    current_turn_history: ThreadHistoryBuilder,
    transcript_metadata_by_item: HashMap<(String, String), TranscriptMetadata>,
    history_mode: ThreadHistoryMode,
    evict_superseded_agent_text: bool,
    listener_thread: Option<Weak<CodexThread>>,
    watch_registration: WatchRegistration,
}

impl ThreadState {
    /// Serializes goal edits, cold resume, and terminal cleanup for this exact state.
    pub(crate) async fn lock_goal_resume(state: &Mutex<Self>) -> tokio::sync::OwnedMutexGuard<()> {
        let lock = Arc::clone(&state.lock().await.goal_resume_lock);
        lock.lock_owned().await
    }

    pub(crate) fn listener_matches(&self, conversation: &Arc<CodexThread>) -> bool {
        self.listener_thread
            .as_ref()
            .and_then(Weak::upgrade)
            .is_some_and(|existing| Arc::ptr_eq(&existing, conversation))
    }

    /// A shut-down runtime can still have terminal events buffered for its listener.
    pub(crate) fn pending_shutdown_drain(&self) -> Option<watch::Receiver<bool>> {
        let drain = self.shutdown_drain.as_ref()?;
        let thread = self.listener_thread.as_ref()?.upgrade()?;
        (thread.has_shutdown_status() && !*drain.borrow() && drain.has_changed().is_ok())
            .then(|| drain.clone())
    }

    pub(crate) fn set_listener(
        &mut self,
        cancel_tx: oneshot::Sender<()>,
        shutdown_drain: watch::Receiver<bool>,
        conversation: &Arc<CodexThread>,
        watch_registration: WatchRegistration,
        thread_settings_baseline: ThreadSettings,
        history_mode: ThreadHistoryMode,
    ) -> (mpsc::UnboundedReceiver<ThreadListenerCommand>, u64) {
        if let Some(previous) = self.cancel_tx.replace(cancel_tx) {
            let _ = previous.send(());
        }
        self.shutdown_drain = Some(shutdown_drain);
        self.listener_generation = self.listener_generation.wrapping_add(1);
        self.last_thread_settings = Some(thread_settings_baseline);
        self.history_mode = history_mode;
        self.evict_superseded_agent_text =
            history_mode == ThreadHistoryMode::Paginated && conversation.rollout_path().is_some();
        let (listener_command_tx, listener_command_rx) = mpsc::unbounded_channel();
        self.listener_command_tx = Some(listener_command_tx);
        self.listener_thread = Some(Arc::downgrade(conversation));
        self.watch_registration = watch_registration;
        (listener_command_rx, self.listener_generation)
    }

    pub(crate) fn clear_listener(&mut self) {
        if let Some(cancel_tx) = self.cancel_tx.take() {
            let _ = cancel_tx.send(());
        }
        self.shutdown_drain = None;
        self.listener_command_tx = None;
        self.current_turn_history.reset();
        self.transcript_metadata_by_item.clear();
        self.listener_thread = None;
        self.watch_registration = WatchRegistration::default();
    }

    pub(crate) fn set_experimental_raw_events(&mut self, enabled: bool) {
        self.experimental_raw_events = enabled;
    }

    pub(crate) fn listener_command_tx(
        &self,
    ) -> Option<mpsc::UnboundedSender<ThreadListenerCommand>> {
        self.listener_command_tx.clone()
    }

    pub(crate) fn active_turn_snapshot(&self) -> Option<Turn> {
        self.current_turn_history.active_turn_snapshot()
    }

    pub(crate) fn runtime_lifecycle_snapshot(&self) -> ThreadRuntimeLifecycle {
        // 生命周期只需要元数据；该快照与完整快照具有相同的“最后完成 turn”回退语义，
        // 但不会复制长回答、图片及工具输出等正文。
        let active_turn = self.active_turn_metadata_snapshot();
        ThreadRuntimeLifecycle {
            active_turn_id: active_turn.as_ref().map(|turn| turn.turn_id.clone()),
            active_turn_started_at: active_turn.and_then(|turn| turn.started_at),
            last_terminal_turn_id: self.last_terminal_turn_id.clone(),
        }
    }

    pub(crate) fn attach_transcript_metadata(&self, turn_id: &str, item: &mut ThreadItem) {
        let item_id = item.id().to_string();
        let metadata = self
            .current_turn_history
            .item_transcript_metadata(turn_id, &item_id)
            .cloned()
            .or_else(|| {
                self.transcript_metadata_by_item
                    .get(&(turn_id.to_string(), item_id))
                    .cloned()
            });
        if let Some(metadata) = metadata {
            item.set_transcript_metadata(metadata);
        }
    }

    /// Returns the same turn ID as `active_turn_snapshot` without cloning its items.
    pub(crate) fn active_turn_id(&self) -> Option<&str> {
        self.current_turn_history.active_turn_id()
    }

    pub(crate) fn active_turn_snapshot_with_items_view(
        &self,
        items_view: TurnItemsView,
    ) -> Option<Turn> {
        self.current_turn_history
            .active_turn_snapshot_with_items_view(items_view)
    }

    pub(crate) fn active_turn_metadata_snapshot(&self) -> Option<ThreadHistoryTurnMetadata> {
        self.current_turn_history.active_turn_metadata_snapshot()
    }

    pub(crate) fn track_current_turn_event(&mut self, event_turn_id: &str, event: &EventMsg) {
        if let EventMsg::TurnStarted(payload) = event {
            self.turn_summary = TurnSummary {
                started_at: payload.started_at,
                ..Default::default()
            };
        }
        if let EventMsg::ItemCompleted(payload) = event
            && let CoreTurnItem::AgentMessage(item) = &payload.item
            && matches!(item.phase, Some(MessagePhase::FinalAnswer) | None)
            && item.content.iter().any(|content| {
                matches!(content, CoreAgentMessageContent::Text { text } if !text.trim().is_empty())
            })
        {
            self.turn_summary.last_agent_message = Some(ThreadItem::from(payload));
        }
        // 只处理本次真正变动的 item。delta/usage 等不修改历史的事件不再复制整段 turn。
        // 保留旧逻辑的两个归属范围，也保留已完成 turn 的缓存，供晚到 completion 使用。
        let changes = if self.evict_superseded_agent_text {
            self.current_turn_history
                .handle_live_paginated_event_with_changes(event)
        } else {
            self.current_turn_history
                .handle_event_with_changes_for_history_mode(event, self.history_mode)
        };
        let metadata_turn_id = transcript_metadata_turn_id(event);
        for change in changes.changed_items {
            if change.turn_id != event_turn_id && metadata_turn_id != Some(change.turn_id.as_str())
            {
                continue;
            }
            if let Some(metadata) = self
                .current_turn_history
                .last_item_transcript_metadata(&change.turn_id, change.item.id())
                .cloned()
            {
                self.transcript_metadata_by_item
                    .insert((change.turn_id, change.item.id().to_string()), metadata);
            }
        }
        if matches!(event, EventMsg::TurnAborted(_) | EventMsg::TurnComplete(_)) {
            self.last_terminal_turn_id = Some(event_turn_id.to_string());
            if !self.current_turn_history.has_active_turn() {
                self.current_turn_history.reset();
            }
        }
    }

    pub(crate) fn note_thread_settings(&mut self, thread_settings: ThreadSettings) -> bool {
        let changed = self.last_thread_settings.as_ref() != Some(&thread_settings);
        self.last_thread_settings = Some(thread_settings);
        changed
    }
}

fn transcript_metadata_turn_id(event: &EventMsg) -> Option<&str> {
    match event {
        EventMsg::ExecCommandBegin(payload) => Some(payload.turn_id.as_str()),
        EventMsg::ExecCommandEnd(payload) => Some(payload.turn_id.as_str()),
        EventMsg::GuardianAssessment(payload) if !payload.turn_id.is_empty() => {
            Some(payload.turn_id.as_str())
        }
        EventMsg::ApplyPatchApprovalRequest(payload) if !payload.turn_id.is_empty() => {
            Some(payload.turn_id.as_str())
        }
        EventMsg::PatchApplyBegin(payload) if !payload.turn_id.is_empty() => {
            Some(payload.turn_id.as_str())
        }
        EventMsg::PatchApplyEnd(payload) if !payload.turn_id.is_empty() => {
            Some(payload.turn_id.as_str())
        }
        EventMsg::ItemStarted(payload) => Some(payload.turn_id.as_str()),
        EventMsg::ItemCompleted(payload) => Some(payload.turn_id.as_str()),
        _ => None,
    }
}

pub(crate) async fn resolve_server_request_on_thread_listener(
    thread_state: &Arc<Mutex<ThreadState>>,
    request_id: RequestId,
) {
    let (completion_tx, completion_rx) = oneshot::channel();
    let listener_command_tx = {
        let state = thread_state.lock().await;
        state.listener_command_tx()
    };
    let Some(listener_command_tx) = listener_command_tx else {
        error!("failed to remove pending client request: thread listener is not running");
        return;
    };

    if listener_command_tx
        .send(ThreadListenerCommand::ResolveServerRequest {
            request_id,
            completion_tx,
        })
        .is_err()
    {
        error!(
            "failed to remove pending client request: thread listener command channel is closed"
        );
        return;
    }

    if let Err(err) = completion_rx.await {
        error!("failed to remove pending client request: {err}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use codex_app_server_protocol::ApprovalsReviewer;
    use codex_app_server_protocol::AskForApproval;
    use codex_app_server_protocol::CommandExecutionSource;
    use codex_app_server_protocol::CommandExecutionStatus;
    use codex_app_server_protocol::SandboxPolicy;
    use codex_protocol::config_types::CollaborationMode;
    use codex_protocol::config_types::ModeKind;
    use codex_protocol::config_types::Settings;
    use codex_protocol::items::CommandExecutionItem;
    use codex_protocol::items::CommandExecutionStatus as CoreCommandExecutionStatus;
    use codex_protocol::items::TurnItem;
    use codex_protocol::protocol::AgentMessageEvent;
    use codex_protocol::protocol::ExecCommandSource;
    use codex_protocol::protocol::ItemStartedEvent;
    use codex_protocol::protocol::TurnCompleteEvent;
    use codex_protocol::protocol::TurnStartedEvent;
    use codex_utils_absolute_path::AbsolutePathBuf;
    use codex_utils_path_uri::PathUri;
    use pretty_assertions::assert_eq;

    #[test]
    fn note_thread_settings_reports_only_effective_changes() {
        let mut state = ThreadState::default();
        let initial = thread_settings("mock-model");
        let updated = thread_settings("mock-model-2");

        let results = vec![
            state.note_thread_settings(initial.clone()),
            state.note_thread_settings(initial),
            state.note_thread_settings(updated.clone()),
            state.note_thread_settings(updated),
        ];

        assert_eq!(results, vec![true, false, true, false]);
    }

    #[tokio::test]
    async fn read_only_thread_state_lookup_does_not_create_an_entry() {
        let manager = ThreadStateManager::new();
        let missing_thread_id = ThreadId::new();

        assert!(
            manager
                .thread_state_if_present(missing_thread_id)
                .await
                .is_none()
        );
        assert!(manager.state.lock().await.threads.is_empty());

        let existing_thread_id = ThreadId::new();
        let existing = manager.thread_state(existing_thread_id).await;
        let resolved = manager
            .thread_state_if_present(existing_thread_id)
            .await
            .expect("existing thread state");
        assert!(Arc::ptr_eq(&existing, &resolved));
        assert_eq!(manager.state.lock().await.threads.len(), 1);
    }

    #[test]
    fn lifecycle_metadata_matches_full_snapshot_through_terminal_and_reset_states() {
        use codex_protocol::protocol::TurnAbortReason;
        use codex_protocol::protocol::TurnAbortedEvent;

        let mut state = ThreadState::default();
        let assert_matches_full_snapshot = |state: &ThreadState| {
            // 参照优化前的完整快照结果，同时覆盖最后完成 turn 的回退行为。
            let full = state.active_turn_snapshot();
            assert_eq!(
                state.runtime_lifecycle_snapshot(),
                ThreadRuntimeLifecycle {
                    active_turn_id: full.as_ref().map(|turn| turn.id.clone()),
                    active_turn_started_at: full.and_then(|turn| turn.started_at),
                    last_terminal_turn_id: state.last_terminal_turn_id.clone(),
                }
            );
        };
        assert_matches_full_snapshot(&state);
        let events = [
            EventMsg::TurnStarted(TurnStartedEvent {
                turn_attribution: None,
                root_turn_id: None,
                turn_id: "turn-large".to_string(),
                trace_id: None,
                started_at: Some(42),
                model_context_window: None,
                collaboration_mode_kind: Default::default(),
            }),
            EventMsg::AgentMessage(AgentMessageEvent {
                moderation: None,
                message: "large answer 汉字\n".repeat(8192),
                questions: None,
                phase: None,
                memory_citation: None,
                delivery: None,
            }),
            EventMsg::TurnComplete(TurnCompleteEvent {
                root_turn_id: None,
                turn_id: "turn-large".to_string(),
                started_at: Some(42),
                last_agent_message: None,
                error: None,
                completed_at: Some(43),
                duration_ms: Some(1000),
                time_to_first_token_ms: None,
            }),
            EventMsg::TurnStarted(TurnStartedEvent {
                turn_attribution: None,
                root_turn_id: None,
                turn_id: "turn-aborted".to_string(),
                trace_id: None,
                started_at: None,
                model_context_window: None,
                collaboration_mode_kind: Default::default(),
            }),
            EventMsg::TurnAborted(TurnAbortedEvent {
                root_turn_id: None,
                turn_id: Some("turn-aborted".to_string()),
                reason: TurnAbortReason::Replaced,
                error: None,
                started_at: None,
                completed_at: None,
                duration_ms: None,
            }),
        ];
        for event in events {
            // 直接驱动 builder，以验证其保留最后完成 turn 时两种快照仍等价；
            // track_current_turn_event 的终止清理由既有 late-command 测试覆盖。
            state.current_turn_history.handle_event(&event);
            assert_matches_full_snapshot(&state);
        }
        state.last_terminal_turn_id = Some("turn-aborted".to_string());
        state.current_turn_history.reset();
        assert_matches_full_snapshot(&state);
    }

    #[test]
    fn transcript_metadata_survives_turn_reset_for_late_command_completion() {
        let mut state = ThreadState::default();
        let thread_id = ThreadId::new();

        state.track_current_turn_event(
            "turn-1",
            &EventMsg::TurnStarted(TurnStartedEvent {
                turn_attribution: None,
                root_turn_id: None,
                turn_id: "turn-1".to_string(),
                trace_id: None,
                started_at: Some(42),
                model_context_window: None,
                collaboration_mode_kind: Default::default(),
            }),
        );
        assert_eq!(
            state.runtime_lifecycle_snapshot(),
            ThreadRuntimeLifecycle {
                active_turn_id: Some("turn-1".to_string()),
                active_turn_started_at: Some(42),
                last_terminal_turn_id: None,
            }
        );
        state.track_current_turn_event(
            "turn-1",
            &EventMsg::AgentMessage(AgentMessageEvent {
                moderation: None,
                message: "I'll run that.".to_string(),
                questions: None,
                phase: None,
                memory_citation: None,
                delivery: None,
            }),
        );
        state.track_current_turn_event(
            "turn-1",
            &EventMsg::ItemStarted(ItemStartedEvent {
                thread_id,
                turn_id: "turn-1".to_string(),
                item: TurnItem::CommandExecution(command_execution_item(
                    "cmd-1",
                    CoreCommandExecutionStatus::InProgress,
                )),
                started_at_ms: 43,
            }),
        );
        state.track_current_turn_event(
            "turn-1",
            &EventMsg::TurnComplete(TurnCompleteEvent {
                root_turn_id: None,
                turn_id: "turn-1".to_string(),
                started_at: None,
                last_agent_message: None,
                error: None,
                completed_at: Some(44),
                duration_ms: Some(2),
                time_to_first_token_ms: None,
            }),
        );

        assert!(state.active_turn_snapshot().is_none());
        assert_eq!(
            state.runtime_lifecycle_snapshot(),
            ThreadRuntimeLifecycle {
                active_turn_id: None,
                active_turn_started_at: None,
                last_terminal_turn_id: Some("turn-1".to_string()),
            }
        );

        let mut item = ThreadItem::untimed(ThreadItemPayload::CommandExecution {
            model_context: None,
            sandbox_type: None,
            id: "cmd-1".to_string(),
            plugin_id: None,
            script_path: None,
            command: "printf hi".to_string(),
            cwd: LegacyAppPathString::from_abs_path(
                &AbsolutePathBuf::from_absolute_path("/tmp").expect("absolute path"),
            ),
            process_id: None,
            source: CommandExecutionSource::Agent,
            status: CommandExecutionStatus::Completed,
            command_actions: Vec::new(),
            aggregated_output: Some("hi".to_string()),
            exit_code: Some(0),
            duration_ms: Some(1),
            transcript_metadata: None,
        });

        state.attach_transcript_metadata("turn-1", &mut item);

        let metadata = item
            .transcript_metadata()
            .expect("late completion should use cached transcript metadata");
        assert_eq!(metadata.turn_id.as_deref(), Some("turn-1"));
        assert_eq!(metadata.backend_item_id.as_deref(), Some("cmd-1"));
        assert_eq!(metadata.order_index, Some(1));
        assert_eq!(metadata.event_sequence, Some(2));
    }

    #[test]
    fn incremental_metadata_matches_full_snapshot_cache_for_each_event() {
        let mut state = ThreadState::default();
        let mut oracle = ThreadHistoryBuilder::new();
        let mut expected = HashMap::new();
        let thread_id = ThreadId::new();
        let mut events = vec![EventMsg::TurnStarted(TurnStartedEvent {
            turn_attribution: None,
            root_turn_id: None,
            turn_id: "turn-1".into(),
            trace_id: None,
            started_at: Some(42),
            model_context_window: None,
            collaboration_mode_kind: Default::default(),
        })];
        // 显式 item-1 与下一条 legacy 自动分配 item-1 碰撞：实时查询 first-wins，
        // 终止后缓存必须保留旧算法的 last-wins，即使第一项后来被 upsert。
        events.push(EventMsg::ItemStarted(ItemStartedEvent {
            thread_id,
            turn_id: "turn-1".into(),
            item: TurnItem::CommandExecution(command_execution_item(
                "item-1",
                CoreCommandExecutionStatus::InProgress,
            )),
            started_at_ms: 43,
        }));
        // 多个带较大正文的 item、重复更新和终止后的晚到完成覆盖缓存与顺序。
        for index in 0..40 {
            events.push(EventMsg::AgentMessage(AgentMessageEvent {
                moderation: None,
                message: format!("{index}:{}", "正文".repeat(1000)),
                questions: None,
                phase: None,
                memory_citation: None,
                delivery: None,
            }));
            if index == 0 {
                events.push(EventMsg::ItemStarted(ItemStartedEvent {
                    thread_id,
                    turn_id: "turn-1".into(),
                    item: TurnItem::CommandExecution(command_execution_item(
                        "item-1",
                        CoreCommandExecutionStatus::InProgress,
                    )),
                    started_at_ms: 43,
                }));
            }
        }
        events.push(EventMsg::ItemStarted(ItemStartedEvent {
            thread_id,
            turn_id: "turn-1".into(),
            item: TurnItem::CommandExecution(command_execution_item(
                "cmd-1",
                CoreCommandExecutionStatus::InProgress,
            )),
            started_at_ms: 43,
        }));
        events.push(EventMsg::ItemCompleted(
            codex_protocol::protocol::ItemCompletedEvent {
                content_token_count: None,
                thread_id,
                turn_id: "turn-1".into(),
                item: TurnItem::CommandExecution(command_execution_item(
                    "item-1",
                    CoreCommandExecutionStatus::Completed,
                )),
                started_at_ms: Some(43),
                completed_at_ms: 44,
            },
        ));
        events.push(EventMsg::TurnComplete(TurnCompleteEvent {
            root_turn_id: None,
            turn_id: "turn-1".into(),
            started_at: Some(42),
            last_agent_message: None,
            error: None,
            completed_at: Some(44),
            duration_ms: Some(2),
            time_to_first_token_ms: None,
        }));
        for event in events {
            oracle.handle_event(&event);
            if let Some(turn) = oracle.turn_snapshot("turn-1") {
                for item in turn.items {
                    if let Some(metadata) = item.transcript_metadata().cloned() {
                        expected.insert(("turn-1".to_string(), item.id().to_string()), metadata);
                    }
                }
            }
            state.track_current_turn_event("turn-1", &event);
            assert_eq!(state.transcript_metadata_by_item, expected);
        }
    }

    fn command_execution_item(
        id: &str,
        status: CoreCommandExecutionStatus,
    ) -> CommandExecutionItem {
        CommandExecutionItem {
            model_context: None,
            sandbox_type: None,
            id: id.to_string(),
            plugin_id: None,
            script_path: None,
            process_id: None,
            command: vec!["printf".to_string(), "hi".to_string()],
            cwd: PathUri::from_abs_path(
                &AbsolutePathBuf::from_absolute_path("/tmp").expect("absolute path"),
            ),
            parsed_cmd: Vec::new(),
            source: ExecCommandSource::Agent,
            interaction_input: None,
            status,

            aggregated_output: None,
            exit_code: None,
            duration: None,
        }
    }

    fn thread_settings(model: &str) -> ThreadSettings {
        ThreadSettings {
            disabled_plugin_ids: Vec::new(),
            environments: Some(Vec::new()),
            cwd: AbsolutePathBuf::from_absolute_path("/tmp").expect("absolute path"),
            approval_policy: AskForApproval::OnRequest,
            approvals_reviewer: ApprovalsReviewer::User,
            sandbox_policy: SandboxPolicy::ReadOnly {
                network_access: false,
            },
            active_permission_profile: None,
            model: model.to_string(),
            model_provider: "mock_provider".to_string(),
            service_tier: None,
            effort: None,
            summary: None,
            collaboration_mode: CollaborationMode {
                mode: ModeKind::Default,
                settings: Settings {
                    model: model.to_string(),
                    reasoning_effort: None,
                    developer_instructions: None,
                },
            },
            multi_agent_mode: MultiAgentMode::ExplicitRequestOnly,
            personality: None,
        }
    }
}

struct ThreadEntry {
    state: Arc<Mutex<ThreadState>>,
    connection_ids: HashSet<ConnectionId>,
    has_connections_watcher: watch::Sender<bool>,
}

impl Default for ThreadEntry {
    fn default() -> Self {
        Self::new(Arc::new(Mutex::new(ThreadState::default())))
    }
}

impl ThreadEntry {
    fn new(state: Arc<Mutex<ThreadState>>) -> Self {
        Self {
            state,
            connection_ids: HashSet::new(),
            has_connections_watcher: watch::channel(false).0,
        }
    }

    fn update_has_connections(&self) {
        let _ = self.has_connections_watcher.send_if_modified(|current| {
            let prev = *current;
            *current = !self.connection_ids.is_empty();
            prev != *current
        });
    }
}

#[derive(Default)]
struct ThreadStateManagerInner {
    live_connections: HashMap<ConnectionId, ConnectionCapabilities>,
    threads: HashMap<ThreadId, ThreadEntry>,
    thread_ids_by_connection: HashMap<ConnectionId, HashSet<ThreadId>>,
}

#[derive(Clone, Copy, Default)]
pub(crate) struct ConnectionCapabilities {
    pub(crate) request_attestation: bool,
}

#[derive(Clone, Default)]
pub(crate) struct ThreadStateManager {
    state: Arc<Mutex<ThreadStateManagerInner>>,
    // Extension event sinks are synchronous, so they need an await-free way to
    // enqueue work on the active per-thread listener.
    listener_commands:
        Arc<StdMutex<HashMap<ThreadId, mpsc::UnboundedSender<ThreadListenerCommand>>>>,
}

/// A listener's state, which may not have been published to the manager yet.
pub(crate) enum ListenerStateCandidate {
    Existing(Arc<Mutex<ThreadState>>),
    Unpublished(Arc<Mutex<ThreadState>>),
}

impl ListenerStateCandidate {
    pub(crate) fn state(&self) -> &Arc<Mutex<ThreadState>> {
        match self {
            Self::Existing(state) | Self::Unpublished(state) => state,
        }
    }
}

impl ThreadStateManager {
    /// Coordinates goal edits with cold/path-based resume for just this thread.
    pub(crate) async fn lock_goal_resume(
        &self,
        thread_id: ThreadId,
    ) -> tokio::sync::OwnedMutexGuard<()> {
        loop {
            let thread_state = self.thread_state(thread_id).await;
            let guard = ThreadState::lock_goal_resume(&thread_state).await;
            // Teardown may have replaced the entry while this caller waited for the lock.
            if self
                .state
                .lock()
                .await
                .threads
                .get(&thread_id)
                .is_some_and(|entry| Arc::ptr_eq(&entry.state, &thread_state))
            {
                return guard;
            }
        }
    }

    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) async fn connection_initialized(
        &self,
        connection_id: ConnectionId,
        capabilities: ConnectionCapabilities,
    ) {
        self.state
            .lock()
            .await
            .live_connections
            .insert(connection_id, capabilities);
    }

    pub(crate) async fn first_attestation_capable_connection_for_thread(
        &self,
        thread_id: ThreadId,
    ) -> Option<ConnectionId> {
        let state = self.state.lock().await;
        state
            .threads
            .get(&thread_id)?
            .connection_ids
            .iter()
            .filter_map(|connection_id| {
                state
                    .live_connections
                    .get(connection_id)?
                    .request_attestation
                    .then_some(*connection_id)
            })
            .min_by_key(|connection_id| connection_id.0)
    }

    pub(crate) async fn wait_for_thread_subscriber(&self, thread_id: ThreadId) {
        let mut has_connections = {
            let mut state = self.state.lock().await;
            state
                .threads
                .entry(thread_id)
                .or_default()
                .has_connections_watcher
                .subscribe()
        };
        while !*has_connections.borrow_and_update() {
            if has_connections.changed().await.is_err() {
                break;
            }
        }
    }

    pub(crate) async fn subscribed_connection_ids(&self, thread_id: ThreadId) -> Vec<ConnectionId> {
        let state = self.state.lock().await;
        state
            .threads
            .get(&thread_id)
            .map(|thread_entry| thread_entry.connection_ids.iter().copied().collect())
            .unwrap_or_default()
    }

    pub(crate) async fn thread_state(&self, thread_id: ThreadId) -> Arc<Mutex<ThreadState>> {
        let mut state = self.state.lock().await;
        state.threads.entry(thread_id).or_default().state.clone()
    }

    pub(crate) async fn thread_state_if_present(
        &self,
        thread_id: ThreadId,
    ) -> Option<Arc<Mutex<ThreadState>>> {
        let state = self.state.lock().await;
        state
            .threads
            .get(&thread_id)
            .map(|thread_entry| thread_entry.state.clone())
    }

    pub(crate) async fn created_listener_state(
        &self,
        thread_id: ThreadId,
    ) -> ListenerStateCandidate {
        match self.state.lock().await.threads.get(&thread_id) {
            Some(entry) => ListenerStateCandidate::Existing(Arc::clone(&entry.state)),
            None => {
                ListenerStateCandidate::Unpublished(Arc::new(Mutex::new(ThreadState::default())))
            }
        }
    }

    pub(crate) fn current_listener_command_tx(
        &self,
        thread_id: ThreadId,
    ) -> Option<mpsc::UnboundedSender<ThreadListenerCommand>> {
        self.listener_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .get(&thread_id)
            .cloned()
    }

    pub(crate) fn register_listener_command_tx(
        &self,
        thread_id: ThreadId,
        tx: mpsc::UnboundedSender<ThreadListenerCommand>,
    ) {
        self.listener_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(thread_id, tx);
    }

    pub(crate) fn unregister_listener_command_tx(&self, thread_id: ThreadId) {
        self.listener_commands
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(&thread_id);
    }

    pub(crate) async fn remove_thread_state(&self, thread_id: ThreadId) {
        let thread_state = {
            let mut state = self.state.lock().await;
            self.unregister_listener_command_tx(thread_id);
            let thread_state = state
                .threads
                .remove(&thread_id)
                .map(|thread_entry| thread_entry.state);
            state.thread_ids_by_connection.retain(|_, thread_ids| {
                thread_ids.remove(&thread_id);
                !thread_ids.is_empty()
            });
            thread_state
        };

        if let Some(thread_state) = thread_state {
            let mut thread_state = thread_state.lock().await;
            tracing::debug!(
                thread_id = %thread_id,
                listener_generation = thread_state.listener_generation,
                had_listener = thread_state.cancel_tx.is_some(),
                had_active_turn = thread_state.active_turn_id().is_some(),
                "clearing thread listener during thread-state teardown"
            );
            thread_state.clear_listener();
        }
    }

    /// Removes an unsubscribed listener's entry without clearing a replacement's state.
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "the listener generation must stay fixed while removing its map entry"
    )]
    pub(crate) async fn remove_unsubscribed_listener(
        &self,
        thread_id: ThreadId,
        expected: &Arc<Mutex<ThreadState>>,
        generation: u64,
    ) -> bool {
        let mut thread_state = expected.lock().await;
        let mut state = self.state.lock().await;
        if thread_state.listener_generation != generation
            || !state.threads.get(&thread_id).is_some_and(|entry| {
                Arc::ptr_eq(&entry.state, expected) && entry.connection_ids.is_empty()
            })
        {
            return false;
        }
        state.threads.remove(&thread_id);
        self.unregister_listener_command_tx(thread_id);
        thread_state.clear_listener();
        true
    }

    pub(crate) async fn clear_all_listeners(&self) {
        let thread_states = {
            let state = self.state.lock().await;
            state
                .threads
                .iter()
                .map(|(thread_id, thread_entry)| (*thread_id, thread_entry.state.clone()))
                .collect::<Vec<_>>()
        };

        for (thread_id, thread_state) in thread_states {
            self.unregister_listener_command_tx(thread_id);
            let mut thread_state = thread_state.lock().await;
            tracing::debug!(
                thread_id = %thread_id,
                listener_generation = thread_state.listener_generation,
                had_listener = thread_state.cancel_tx.is_some(),
                had_active_turn = thread_state.active_turn_id().is_some(),
                "clearing thread listener during app-server shutdown"
            );
            thread_state.clear_listener();
        }
    }

    pub(crate) async fn unsubscribe_connection_from_thread(
        &self,
        thread_id: ThreadId,
        connection_id: ConnectionId,
    ) -> bool {
        {
            let mut state = self.state.lock().await;
            if !state
                .thread_ids_by_connection
                .get(&connection_id)
                .is_some_and(|thread_ids| thread_ids.contains(&thread_id))
            {
                return false;
            }

            if let Some(thread_ids) = state.thread_ids_by_connection.get_mut(&connection_id) {
                thread_ids.remove(&thread_id);
                if thread_ids.is_empty() {
                    state.thread_ids_by_connection.remove(&connection_id);
                }
            }
            if let Some(thread_entry) = state.threads.get_mut(&thread_id) {
                thread_entry.connection_ids.remove(&connection_id);
                thread_entry.update_has_connections();
            }
        };

        true
    }

    pub(crate) async fn has_subscribers(&self, thread_id: ThreadId) -> bool {
        self.state
            .lock()
            .await
            .threads
            .get(&thread_id)
            .is_some_and(|thread_entry| !thread_entry.connection_ids.is_empty())
    }

    /// Publishes a new state only when listener installation succeeds, or reuses a current state.
    /// The caller must hold the candidate's state lock.
    pub(crate) async fn subscribe_and_install_listener<T, E>(
        &self,
        thread_id: ThreadId,
        candidate: &ListenerStateCandidate,
        connection_ids: &[ConnectionId],
        install: impl FnOnce(bool) -> Result<T, E>,
    ) -> Result<Option<(T, watch::Receiver<bool>, bool)>, E> {
        let mut state = self.state.lock().await;
        let ThreadStateManagerInner {
            live_connections,
            threads,
            thread_ids_by_connection,
        } = &mut *state;
        let slot = threads.entry(thread_id);
        match (&slot, candidate) {
            (Entry::Occupied(entry), _) if Arc::ptr_eq(&entry.get().state, candidate.state()) => {}
            (Entry::Vacant(_), ListenerStateCandidate::Unpublished(_)) => {}
            _ => return Ok(None),
        }
        let attached = connection_ids
            .iter()
            .any(|connection_id| live_connections.contains_key(connection_id));
        let installed = install(attached)?;
        let entry = slot.or_insert_with(|| ThreadEntry::new(Arc::clone(candidate.state())));
        for &connection_id in connection_ids {
            if live_connections.contains_key(&connection_id) {
                thread_ids_by_connection
                    .entry(connection_id)
                    .or_default()
                    .insert(thread_id);
                entry.connection_ids.insert(connection_id);
            }
        }
        entry.update_has_connections();
        Ok(Some((
            installed,
            entry.has_connections_watcher.subscribe(),
            attached,
        )))
    }

    pub(crate) async fn try_add_connection_to_thread(
        &self,
        thread_id: ThreadId,
        connection_id: ConnectionId,
    ) -> bool {
        let mut state = self.state.lock().await;
        if !state.live_connections.contains_key(&connection_id) {
            return false;
        }
        state
            .thread_ids_by_connection
            .entry(connection_id)
            .or_default()
            .insert(thread_id);
        let thread_entry = state.threads.entry(thread_id).or_default();
        thread_entry.connection_ids.insert(connection_id);
        thread_entry.update_has_connections();
        true
    }

    pub(crate) async fn remove_connection(&self, connection_id: ConnectionId) -> Vec<ThreadId> {
        {
            let mut state = self.state.lock().await;
            state.live_connections.remove(&connection_id);
            let thread_ids = state
                .thread_ids_by_connection
                .remove(&connection_id)
                .unwrap_or_default();
            for thread_id in &thread_ids {
                if let Some(thread_entry) = state.threads.get_mut(thread_id) {
                    thread_entry.connection_ids.remove(&connection_id);
                    thread_entry.update_has_connections();
                }
            }
            thread_ids
                .into_iter()
                .filter(|thread_id| {
                    state
                        .threads
                        .get(thread_id)
                        .is_some_and(|thread_entry| thread_entry.connection_ids.is_empty())
                })
                .collect::<Vec<_>>()
        }
    }

    #[cfg(test)]
    pub(crate) async fn subscribe_to_has_connections(
        &self,
        thread_id: ThreadId,
    ) -> Option<watch::Receiver<bool>> {
        let state = self.state.lock().await;
        state
            .threads
            .get(&thread_id)
            .map(|thread_entry| thread_entry.has_connections_watcher.subscribe())
    }
}
