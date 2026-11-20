use super::ResponsesStreamRequest;
use super::ResponsesStreamRetryState;
use super::handle_response_stream_error;
use super::log_retry;
use crate::realtime_history::RealtimeHistoryState;
use crate::session::step_context::StepContext;
use crate::session::tests::make_session_and_context;
use codex_http_client::RetryAfter;
use codex_protocol::error::CodexErr;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;
use tracing_test::internal::MockWriter;

#[tokio::test]
async fn sampling_retry_logs_stream_error_context() {
    let (_session, turn_context) = make_session_and_context().await;
    let buffer: &'static std::sync::Mutex<Vec<u8>> =
        Box::leak(Box::new(std::sync::Mutex::new(Vec::new())));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .with_max_level(tracing::Level::WARN)
        .with_writer(MockWriter::new(buffer))
        .finish();
    let _subscriber_guard = tracing::subscriber::set_default(subscriber);

    log_retry(
        ResponsesStreamRequest::Sampling,
        &turn_context,
        &CodexErr::Stream("websocket closed by server before response.completed".to_string()),
        /*retries*/ 2,
        /*max_retries*/ 5,
        Duration::from_secs(1),
    );

    let logs = String::from_utf8(
        buffer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone(),
    )
    .expect("retry log should be valid utf-8");
    assert!(logs.contains("stream disconnected - retrying sampling request"));
    assert!(logs.contains(&format!("turn_id={}", turn_context.sub_id)));
    assert!(logs.contains("retries=2"));
    assert!(logs.contains("max_retries=5"));
    assert!(logs.contains(
        "sampling_error=stream disconnected before completion: websocket closed by server before response.completed"
    ));
}

/// Time spent reporting a retry must count toward the server's original deadline.
#[tokio::test]
#[expect(
    clippy::await_holding_invalid_type,
    reason = "test holds the event-delivery lock to delay notification while virtual time advances"
)]
async fn stream_retry_preserves_deadline_across_delayed_notification() {
    let (mut session, turn_context) = make_session_and_context().await;
    let step_context = StepContext::for_test(Arc::new(turn_context));
    session.realtime_history = Some(Mutex::new(RealtimeHistoryState::default()));
    let mut client_session = session.services.model_client.new_session();
    // 首次重试在 release/debug 都必须发布通知，锁模拟延迟交付。
    let mut retry_state = ResponsesStreamRetryState {
        retries: 0,
        ..Default::default()
    };

    tokio::time::pause();
    let advice = RetryAfter::from_delay(Duration::from_secs(10)).expect("retry deadline");
    tokio::time::advance(Duration::from_secs(4)).await;

    // Hold the event-delivery lock so reporting the error consumes part of the deadline.
    let history_guard = session.realtime_history.as_ref().unwrap().lock().await;
    let retry = handle_response_stream_error(
        &mut retry_state,
        /*max_retries*/ 2,
        CodexErr::InternalServerError.with_retry_after(advice),
        &mut client_session,
        &session,
        &step_context,
        ResponsesStreamRequest::Sampling,
    );
    tokio::pin!(retry);
    assert!(futures::poll!(&mut retry).is_pending());
    tokio::time::advance(Duration::from_secs(4)).await;
    drop(history_guard);

    assert!(futures::poll!(&mut retry).is_pending());
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(futures::poll!(&mut retry).is_pending());
    retry.await.expect("retry should be allowed");
    // Tokio rounds timer deadlines up to the next millisecond.
    let resumed_at = Instant::now();
    assert!(
        (advice.deadline()..=advice.deadline() + Duration::from_millis(1)).contains(&resumed_at),
        "retry resumed at {resumed_at:?}, expected {advice:?}"
    );
}

/// 恢复预算耗尽后不能继续等待或因切换传输重新获得预算。
#[tokio::test]
async fn expired_recovery_budget_returns_original_error_without_retry() {
    let (session, turn_context) = make_session_and_context().await;
    let step_context = StepContext::for_test(Arc::new(turn_context));
    let mut client = session.services.model_client.new_session();
    tokio::time::pause();
    let started_at = Instant::now();
    tokio::time::advance(super::MAX_RECOVERY_WAIT).await;
    let mut state = ResponsesStreamRetryState {
        recovery_started_at: Some(started_at),
        ..Default::default()
    };
    let result = handle_response_stream_error(
        &mut state,
        5,
        CodexErr::Stream("synthetic disconnect".into()),
        &mut client,
        &session,
        &step_context,
        ResponsesStreamRequest::Sampling,
    )
    .await;
    assert!(result.is_err());
    assert_eq!(state.retries, 0);
    assert_eq!(state.connection_retries, 0);
}

/// 超预算的 Retry-After 必须返回失败，不能提前请求或无限等候。
#[tokio::test]
async fn server_retry_deadline_cannot_extend_recovery_budget() {
    tokio::time::pause();
    let mut state = ResponsesStreamRetryState::default();
    let advice = RetryAfter::from_delay(super::MAX_RECOVERY_WAIT + Duration::from_secs(1));
    assert!(
        state
            .recovery_deadline(Duration::from_secs(1), advice)
            .is_none()
    );
}

/// 网络恢复也必须遵守服务端 Retry-After，不能用本地五秒退避提前重发。
#[tokio::test]
async fn connection_retry_honors_server_advice_without_shortening_local_backoff() {
    tokio::time::pause();
    let state = ResponsesStreamRetryState::default();
    let now = Instant::now();
    let later = RetryAfter::from_delay(Duration::from_secs(30));
    assert_eq!(
        state.connection_retry_at(now, later),
        now + Duration::from_secs(30)
    );
    let earlier = RetryAfter::from_delay(Duration::from_secs(1));
    assert_eq!(
        state.connection_retry_at(now, earlier),
        now + Duration::from_secs(5)
    );
}

#[tokio::test]
async fn recovery_metadata_preserves_request_identity_and_reports_real_deadlines() {
    let state = ResponsesStreamRetryState::default();
    let now = Instant::now();
    let status = state.recovery_status(
        2,
        Some(4),
        now + Duration::from_secs(10),
        now + Duration::from_secs(60),
        "connection",
    );
    assert_eq!(status.attempt, 2);
    assert_eq!(status.max_attempts, Some(4));
    assert!((49_999..=50_001).contains(&(status.budget_deadline_ms - status.retry_at_ms)));
    let switched = state.recovery_status(5, None, now, now + Duration::from_secs(60), "transport");
    assert_eq!(switched.request_id, status.request_id);
    assert_eq!(switched.max_attempts, None);
    assert_ne!(
        ResponsesStreamRetryState::default().request_id,
        status.request_id
    );
}

/// 通知交付阻塞跨过总预算后，必须返回原错误，不能再准入请求。
#[tokio::test]
#[expect(
    clippy::await_holding_invalid_type,
    reason = "用交付锁模拟预算期间通知阻塞"
)]
async fn notification_delay_cannot_admit_retry_after_budget_expires() {
    for branch in ["sampling", "connection", "transport"] {
        let (mut session, turn_context) = make_session_and_context().await;
        let step_context = StepContext::for_test(Arc::new(turn_context));
        session.realtime_history = Some(Mutex::new(RealtimeHistoryState::default()));
        let mut client = session.services.model_client.new_session();
        let mut state = ResponsesStreamRetryState::default();
        tokio::time::pause();
        let deadline = Instant::now() + super::MAX_RECOVERY_WAIT;
        let advice = RetryAfter::from_delay(Duration::from_secs(10)).unwrap();
        let history_guard = session.realtime_history.as_ref().unwrap().lock().await;
        // 三条等待路径共用真实交付阻塞，确保都在唤醒后执行预算准入检查。
        let retry = async {
            let error = CodexErr::InternalServerError.with_retry_after(advice);
            match branch {
                "connection" => {
                    super::retry_connection(
                        &mut state,
                        &session,
                        &step_context.turn,
                        error,
                        Some(advice),
                        deadline,
                        codex_client::RetryOperation::Sampling,
                    )
                    .await
                }
                "transport" => {
                    super::retry_after_transport_switch(
                        &state,
                        &session,
                        &step_context.turn,
                        error,
                        Some(advice),
                        deadline,
                    )
                    .await
                }
                _ => {
                    handle_response_stream_error(
                        &mut state,
                        2,
                        error,
                        &mut client,
                        &session,
                        &step_context,
                        ResponsesStreamRequest::Sampling,
                    )
                    .await
                }
            }
        };
        tokio::pin!(retry);
        assert!(futures::poll!(&mut retry).is_pending());
        tokio::time::advance(super::MAX_RECOVERY_WAIT).await;
        drop(history_guard);
        let error = retry.await.expect_err("过期通知不能触发新请求");
        assert_eq!(error.to_string(), CodexErr::InternalServerError.to_string());
        assert_eq!(error.retry_after().unwrap().deadline(), advice.deadline());
        tokio::time::resume();
    }
}

/// 进程休眠造成退避唤醒超过预算时，也不能再发送请求。
#[tokio::test]
async fn backoff_wakeup_after_deadline_returns_original_error() {
    let (session, turn_context) = make_session_and_context().await;
    let step_context = StepContext::for_test(Arc::new(turn_context));
    let mut client = session.services.model_client.new_session();
    let mut state = ResponsesStreamRetryState::default();
    tokio::time::pause();
    let retry = handle_response_stream_error(
        &mut state,
        2,
        CodexErr::InternalServerError,
        &mut client,
        &session,
        &step_context,
        ResponsesStreamRequest::Sampling,
    );
    tokio::pin!(retry);
    assert!(futures::poll!(&mut retry).is_pending());
    tokio::time::advance(super::MAX_RECOVERY_WAIT).await;
    assert_eq!(
        retry.await.unwrap_err().to_string(),
        CodexErr::InternalServerError.to_string()
    );
}
