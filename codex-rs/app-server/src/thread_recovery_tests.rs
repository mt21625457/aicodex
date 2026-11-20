//! 按监听器真实顺序验证快照，状态查询不能把后台恢复伪装成已完成。
use super::ThreadState;
use codex_protocol::protocol::AgentMessageContentDeltaEvent;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::StreamErrorEvent;
use codex_protocol::protocol::TurnStartedEvent;
use codex_protocol::stream_recovery::StreamRecovery;

fn start(turn: &str) -> EventMsg {
    EventMsg::TurnStarted(TurnStartedEvent {
        turn_attribution: None,
        root_turn_id: None,
        turn_id: turn.into(),
        trace_id: None,
        started_at: Some(1),
        model_context_window: None,
        collaboration_mode_kind: Default::default(),
    })
}
fn retry() -> EventMsg {
    EventMsg::StreamError(StreamErrorEvent {
        message: "retry".into(),
        codex_error_info: None,
        additional_details: None,
        recovery: Some(StreamRecovery {
            extra: Default::default(),
            request_id: "local-request".into(),
            attempt: 1,
            max_attempts: Some(3),
            retry_at_ms: 2000,
            budget_deadline_ms: 3000,
            reason: "connection".into(),
        }),
    })
}
fn delta(turn: &str) -> EventMsg {
    EventMsg::AgentMessageContentDelta(AgentMessageContentDeltaEvent {
        thread_id: "thread".into(),
        turn_id: turn.into(),
        item_id: "item".into(),
        delta: "ok".into(),
        phase: None,
    })
}

#[test]
fn recovery_snapshot_survives_queries_but_not_real_progress() {
    let mut state = ThreadState::default();
    state.track_current_turn_event("turn", &start("turn"));
    state.track_current_turn_event("turn", &retry());
    assert_eq!(
        state.runtime_lifecycle_snapshot().recovery.unwrap().attempt,
        1
    );
    assert!(state.runtime_lifecycle_snapshot().recovery.is_some());
    state.track_current_turn_event("old", &delta("old"));
    assert!(state.runtime_lifecycle_snapshot().recovery.is_some());
    state.track_current_turn_event("turn", &delta("turn"));
    assert!(state.runtime_lifecycle_snapshot().recovery.is_none());
}

#[test]
fn replacement_and_listener_shutdown_clear_recovery() {
    let mut state = ThreadState::default();
    state.track_current_turn_event("turn", &start("turn"));
    state.track_current_turn_event("turn", &retry());
    state.track_current_turn_event("new", &start("new"));
    state.track_current_turn_event("turn", &retry());
    assert!(state.runtime_lifecycle_snapshot().recovery.is_none());
    state.track_current_turn_event("new", &retry());
    state.clear_listener();
    assert!(state.runtime_lifecycle_snapshot().recovery.is_none());
}

#[test]
fn concurrent_tool_completion_does_not_claim_provider_recovered() {
    use codex_protocol::items::FunctionCallOutputItem;
    use codex_protocol::items::TurnItem;
    use codex_protocol::models::FunctionCallOutputBody;
    use codex_protocol::protocol::ItemCompletedEvent;
    let mut state = ThreadState::default();
    state.track_current_turn_event("turn", &start("turn"));
    state.track_current_turn_event("turn", &retry());
    let completed = EventMsg::ItemCompleted(ItemCompletedEvent {
        thread_id: codex_protocol::ThreadId::new(),
        turn_id: "turn".into(),
        item: TurnItem::FunctionCallOutput(FunctionCallOutputItem {
            id: "tool".into(),
            name: "shell".into(),
            namespace: None,
            output: FunctionCallOutputBody::Text("done".into()),
        }),
        content_token_count: None,
        started_at_ms: None,
        completed_at_ms: 2000,
    });
    state.track_current_turn_event("turn", &completed);
    assert!(state.runtime_lifecycle_snapshot().recovery.is_some());
}

#[test]
fn terminal_turn_cannot_be_reopened_by_late_retry_evidence() {
    let mut state = ThreadState::default();
    state.track_current_turn_event("turn", &start("turn"));
    state.track_current_turn_event("turn", &retry());
    let aborted =
        serde_json::from_value(serde_json::json!({"turn_id":"turn", "reason":"interrupted"}))
            .unwrap();
    state.track_current_turn_event("turn", &EventMsg::TurnAborted(aborted));
    state.track_current_turn_event("turn", &retry());
    let snapshot = state.runtime_lifecycle_snapshot();
    assert!(snapshot.recovery.is_none());
    assert!(snapshot.active_turn_id.is_none());
}

#[test]
fn tool_input_generation_clears_retry_but_cleanup_does_not() {
    use codex_protocol::tool_input_progress::ToolInputProgressEvent;
    let mut state = ThreadState::default();
    state.track_current_turn_event("turn", &start("turn"));
    state.track_current_turn_event("turn", &retry());
    let progress = |finished| {
        EventMsg::ToolInputProgress(ToolInputProgressEvent {
            item_id: "tool".into(),
            input_bytes: 0,
            finished,
        })
    };
    // finished 同时用于错误清理，不能被当成连接恢复；新调用的零字节开始则是实际流事件。
    state.track_current_turn_event("turn", &progress(true));
    assert!(state.runtime_lifecycle_snapshot().recovery.is_some());
    state.track_current_turn_event("old", &progress(false));
    assert!(state.runtime_lifecycle_snapshot().recovery.is_some());
    state.track_current_turn_event("turn", &progress(false));
    assert!(state.runtime_lifecycle_snapshot().recovery.is_none());
}
