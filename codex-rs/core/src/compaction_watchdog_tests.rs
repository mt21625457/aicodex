use super::*;
use codex_protocol::error::CodexErrorDetails;
use std::future::pending;

#[tokio::test(start_paused = true)]
async fn silent_connection_times_out_without_waiting_for_provider_hour_limit() {
    let watchdog = CompactionWatchdog::new();
    let start = Instant::now();
    let error = watchdog
        .wait("connecting", pending::<()>())
        .await
        .unwrap_err();
    assert!(matches!(
        error.details(),
        CodexErrorDetails::CompactionTimeout
    ));
    assert_eq!(start.elapsed(), IDLE_LIMIT);
}

#[tokio::test(start_paused = true)]
async fn metadata_and_empty_deltas_do_not_extend_idle_budget() {
    let mut watchdog = CompactionWatchdog::new();
    tokio::time::advance(Duration::from_secs(110)).await;
    watchdog.observe(&ResponseEvent::ModelsEtag("changed".into()));
    watchdog.observe(&ResponseEvent::OutputTextDelta(String::new()));
    let start = Instant::now();
    assert!(
        watchdog
            .wait("awaiting_summary", pending::<()>())
            .await
            .is_err()
    );
    assert_eq!(start.elapsed(), Duration::from_secs(10));
}

#[tokio::test(start_paused = true)]
async fn real_progress_extends_idle_after_soft_deadline() {
    let mut watchdog = CompactionWatchdog::new();
    for _ in 0..2 {
        tokio::time::advance(Duration::from_secs(100)).await;
        watchdog.observe(&ResponseEvent::OutputTextDelta("summary".into()));
        assert_eq!(
            watchdog
                .wait("awaiting_summary", async { 42 })
                .await
                .unwrap(),
            42
        );
    }
    let start = Instant::now();
    assert!(
        watchdog
            .wait("awaiting_summary", pending::<()>())
            .await
            .is_err()
    );
    assert_eq!(start.elapsed(), Duration::from_secs(120));
}

#[tokio::test(start_paused = true)]
async fn expired_budget_cannot_be_reset_by_a_ready_retry() {
    let watchdog = CompactionWatchdog::new();
    tokio::time::advance(IDLE_LIMIT).await;
    assert!(watchdog.wait("connecting", async { 42 }).await.is_err());
}

/// 已持续生成的同一请求可越过软截止，避免第三次摘要仍在生成时被截断。
#[tokio::test(start_paused = true)]
async fn active_summary_survives_soft_deadline() {
    let mut watchdog = CompactionWatchdog::new();
    for _ in 0..3 {
        tokio::time::advance(Duration::from_secs(99)).await;
        watchdog.observe(&ResponseEvent::OutputTextDelta("synthetic summary".into()));
    }
    tokio::time::advance(Duration::from_secs(4)).await;
    assert!(
        watchdog
            .wait("awaiting_summary", async { () })
            .await
            .is_ok()
    );
}

/// 永久输出不能绕过硬截止，且已就绪值也不能在截止后冒充成功。
#[tokio::test(start_paused = true)]
async fn continuous_output_stops_at_global_hard_limit() {
    let mut watchdog = CompactionWatchdog::new();
    for _ in 0..6 {
        tokio::time::advance(Duration::from_secs(99)).await;
        watchdog.observe(&ResponseEvent::OutputTextDelta("synthetic".into()));
    }
    let start = Instant::now();
    assert!(
        watchdog
            .wait("awaiting_summary", pending::<()>())
            .await
            .is_err()
    );
    assert_eq!(start.elapsed(), Duration::from_secs(6));
    assert!(watchdog.wait("awaiting_summary", async {}).await.is_err());
}

#[tokio::test(start_paused = true)]
async fn delayed_poll_preserves_the_first_expired_timeout_kind() {
    use codex_protocol::compaction_error::CompactionTimeoutKind;
    use codex_protocol::protocol::CodexErrorInfo;

    let mut watchdog = CompactionWatchdog::new();
    watchdog.begin_attempt().unwrap();
    for _ in 0..5 {
        tokio::time::advance(Duration::from_secs(110)).await;
        watchdog.observe(&ResponseEvent::OutputTextDelta("synthetic".into()));
    }
    // 硬截止在600秒，idle截止在670秒；模拟executor到700秒才恢复调度。
    tokio::time::advance(Duration::from_secs(150)).await;
    let error = watchdog
        .wait("awaiting_summary", async {})
        .await
        .unwrap_err();
    let CodexErrorInfo::CompactionTimeout {
        context: Some(context),
    } = error.to_codex_protocol_error()
    else {
        panic!("expected typed compaction timeout");
    };
    assert_eq!(context.kind, CompactionTimeoutKind::Total);
    assert_eq!(context.attempt, Some(1));
}

#[tokio::test(start_paused = true)]
async fn two_retries_share_budget_and_insufficient_budget_sends_no_attempt() {
    let mut watchdog = CompactionWatchdog::new();
    watchdog.begin_attempt().unwrap();
    tokio::time::advance(Duration::from_secs(70)).await;
    watchdog.admit_retry(Duration::from_secs(2)).unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    watchdog.begin_attempt().unwrap();
    tokio::time::advance(Duration::from_secs(70)).await;
    watchdog.admit_retry(Duration::from_secs(2)).unwrap();
    tokio::time::advance(Duration::from_secs(2)).await;
    watchdog.begin_attempt().unwrap();
    tokio::time::advance(Duration::from_secs(37)).await;
    assert!(watchdog.admit_retry(Duration::ZERO).is_err());
    assert!(watchdog.begin_attempt().is_err());
    assert_eq!(watchdog.attempt, 3);
}

#[tokio::test(start_paused = true)]
async fn retry_admission_reserves_backoff_and_idle_window() {
    let watchdog = CompactionWatchdog::new();
    tokio::time::advance(Duration::from_secs(178)).await;
    assert!(watchdog.admit_retry(Duration::from_secs(2)).is_ok());
    assert!(watchdog.admit_retry(Duration::from_secs(3)).is_err());
}

#[tokio::test(start_paused = true)]
async fn previous_attempt_progress_cannot_extend_new_attempt() {
    let mut watchdog = CompactionWatchdog::new();
    watchdog.begin_attempt().unwrap();
    tokio::time::advance(Duration::from_secs(100)).await;
    watchdog.observe(&ResponseEvent::OutputTextDelta("old attempt".into()));
    watchdog.begin_attempt().unwrap();
    let start = Instant::now();
    assert!(watchdog.wait("connecting", pending::<()>()).await.is_err());
    assert_eq!(start.elapsed(), IDLE_LIMIT);
}

/// 开始输出项但尚无正文不能让空流取得进展宽限。
#[tokio::test(start_paused = true)]
async fn empty_output_item_start_does_not_extend_idle() {
    let mut watchdog = CompactionWatchdog::new();
    tokio::time::advance(Duration::from_secs(110)).await;
    let item = serde_json::from_value(serde_json::json!({
        "type":"message","role":"assistant","content":[]
    }))
    .unwrap();
    watchdog.observe(&ResponseEvent::OutputItemAdded(item));
    let start = Instant::now();
    assert!(
        watchdog
            .wait("awaiting_summary", pending::<()>())
            .await
            .is_err()
    );
    assert_eq!(start.elapsed(), Duration::from_secs(10));
}

#[test_case::test_case(serde_json::json!({"type":"message","role":"assistant","content":[]}), false; "empty_message")]
#[test_case::test_case(serde_json::json!({"type":"reasoning","summary":[]}), false; "empty_reasoning")]
#[test_case::test_case(serde_json::json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"summary"}]}), true; "whole_message_without_deltas")]
#[test_case::test_case(serde_json::json!({"type":"reasoning","summary":[{"type":"summary_text","text":"progress"}]}), true; "whole_reasoning_without_deltas")]
#[tokio::test(start_paused = true)]
async fn completed_item_requires_content_to_extend_idle(item: serde_json::Value, meaningful: bool) {
    let mut watchdog = CompactionWatchdog::new();
    tokio::time::advance(Duration::from_secs(110)).await;
    watchdog.observe(&ResponseEvent::OutputItemDone(
        serde_json::from_value(item).unwrap(),
    ));
    let start = Instant::now();
    assert!(
        watchdog
            .wait("awaiting_summary", pending::<()>())
            .await
            .is_err()
    );
    let expected = if meaningful { 120 } else { 10 };
    assert_eq!(start.elapsed(), Duration::from_secs(expected));
}
