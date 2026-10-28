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
async fn real_progress_extends_idle_but_never_total_budget() {
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
    assert_eq!(start.elapsed(), Duration::from_secs(100));
}

#[tokio::test(start_paused = true)]
async fn expired_budget_cannot_be_reset_by_a_ready_retry() {
    let watchdog = CompactionWatchdog::new();
    tokio::time::advance(IDLE_LIMIT).await;
    assert!(watchdog.wait("connecting", async { 42 }).await.is_err());
}
