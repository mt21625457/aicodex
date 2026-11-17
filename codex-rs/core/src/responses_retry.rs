//! Shared retry and transport fallback decisions for Responses requests.
//! Content-filter guidance is recorded for sampling requests before retry decisions.
//! Server advice controls timing without extending configured retry limits.

use std::time::Duration;

use crate::client::ModelClientSession;
use crate::context::ContentFilterGuidance;
use crate::context::ContextualUserFragment;
use crate::session::session::Session;
use crate::session::step_context::StepContext;
use crate::session::turn_context::TurnContext;
use codex_client::RetryOperation;
use codex_features::Feature;
use codex_http_client::RetryAfter;
use codex_protocol::error::CodexErr;
use codex_protocol::error::CodexErrorDetails;
use tokio::time::Instant;
use tracing::warn;

// 恢复预算从首次可重试失败开始，跨传输切换保留；不限制正常工具执行时间。
const MAX_RECOVERY_WAIT: Duration = Duration::from_secs(15 * 60);

const INITIAL_CONNECTION_RETRY_DELAY: Duration = Duration::from_secs(5);
const MAX_CONNECTION_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy)]
pub(crate) enum ResponsesStreamRequest {
    Sampling,
    RemoteCompactionV2,
}

pub(crate) struct ResponsesStreamRetryState {
    recovery_started_at: Option<Instant>,
    retries: u64,
    connection_retries: u64,
    connection_retry_delay: Duration,
}

impl Default for ResponsesStreamRetryState {
    fn default() -> Self {
        Self {
            recovery_started_at: None,
            retries: 0,
            connection_retries: 0,
            connection_retry_delay: INITIAL_CONNECTION_RETRY_DELAY,
        }
    }
}

impl ResponsesStreamRetryState {
    /// 连接退避和服务端建议都必须满足；通知交付耗时算入同一个绝对期限。
    fn connection_retry_at(&self, advice: Option<RetryAfter>) -> Instant {
        let local = Instant::now() + self.connection_retry_delay;
        advice
            .map(RetryAfter::deadline)
            .map_or(local, |remote| remote.max(local))
    }

    /// 单次请求恢复的时限由首次失败固定；服务器等待不能被截短或跨传输重置。
    fn recovery_deadline(
        &mut self,
        delay: Duration,
        retry_after: Option<RetryAfter>,
    ) -> Option<Instant> {
        let now = Instant::now();
        let started = *self.recovery_started_at.get_or_insert(now);
        let deadline = started + MAX_RECOVERY_WAIT;
        let retry_at = retry_after.map(RetryAfter::deadline).unwrap_or(now + delay);
        (now < deadline && retry_at < deadline).then_some(deadline)
    }
}

/// Server retry advice retained after stream retries are exhausted. The turn ID
/// prevents a reused Guardian session from applying advice from an earlier review.
pub(crate) struct ExhaustedResponseRetry {
    pub(crate) turn_id: String,
    pub(crate) retry_at: Option<tokio::time::Instant>,
}

/// Returns `Ok(())` when the caller should retry the request loop, or the original error when
/// it is terminal or the retry budget is exhausted.
#[tracing::instrument(level = "trace", skip_all, fields(request = ?request))]
pub(crate) async fn handle_response_stream_error(
    retry_state: &mut ResponsesStreamRetryState,
    max_retries: u64,
    err: CodexErr,
    client_session: &mut ModelClientSession,
    sess: &Session,
    step_context: &StepContext,
    request: ResponsesStreamRequest,
) -> Result<(), CodexErr> {
    let turn_context = &step_context.turn;
    if matches!(request, ResponsesStreamRequest::Sampling)
        && matches!(err.details(), CodexErrorDetails::ContentFilter)
    {
        record_content_filter_guidance(sess, step_context).await;
    }
    let operation = match request {
        ResponsesStreamRequest::Sampling => RetryOperation::Sampling,
        ResponsesStreamRequest::RemoteCompactionV2 => RetryOperation::RemoteCompactionV2,
    };
    let retry_count = retry_state.retries.saturating_add(1);
    let Some(delay) = err.retry_delay(retry_count) else {
        return Err(err);
    };
    let retry_after = err.retry_after();
    let Some(deadline) = retry_state.recovery_deadline(delay, retry_after) else {
        warn!(
            code = "stream.recovery_budget_exhausted",
            "response recovery budget exhausted"
        );
        record_exhausted_retry(sess, turn_context, retry_after);
        return Err(err);
    };

    if turn_context
        .config
        .features
        .enabled(Feature::UnboundedConnectionRetries)
        && matches!(request, ResponsesStreamRequest::Sampling)
        && matches!(err.details(), CodexErrorDetails::ConnectionFailed(_))
        && !turn_context.session_source.is_internal()
        && !turn_context.provider.info().is_amazon_bedrock()
    {
        let retry_at = retry_state.connection_retry_at(retry_after);
        let retry_delay = retry_at.saturating_duration_since(Instant::now());
        if retry_at >= deadline {
            record_exhausted_retry(sess, turn_context, retry_after);
            return Err(err);
        }
        warn!(
            turn_id = %turn_context.sub_id,
            error = %err,
            ?retry_delay,
            "stream connection failed; waiting to retry"
        );
        sess.notify_stream_error(turn_context, "Reconnecting... waiting for network", err)
            .await;
        retry_state.connection_retries = retry_state.connection_retries.saturating_add(1);
        codex_client::record_retry!(retry_state.connection_retries, retry_delay, operation);
        tokio::time::sleep_until(retry_at).await;
        retry_state.connection_retry_delay = retry_state
            .connection_retry_delay
            .saturating_mul(2)
            .min(MAX_CONNECTION_RETRY_DELAY);
        return Ok(());
    }

    if retry_state.retries >= max_retries
        && client_session.try_switch_fallback_transport(
            &turn_context.session_telemetry,
            turn_context.model_info(),
        )
    {
        // 切换传输仍属于恢复，必须通知状态栏；累计次数与时间预算不能重置。
        warn!(
            turn_id = %turn_context.sub_id,
            error = %err,
            "Falling back from WebSockets to HTTPS transport"
        );
        sess.notify_stream_error(turn_context, "Reconnecting... switching transport", err)
            .await;
        // 先发布恢复状态，再遵守服务器等待时限，避免等待期间继续显示普通 Working。
        if let Some(retry_after) = retry_after {
            tokio::time::sleep_until(retry_after.deadline()).await;
        }
        return Ok(());
    }

    if retry_state.retries < max_retries {
        retry_state.retries = retry_count;
        log_retry(request, turn_context, &err, retry_count, max_retries, delay);

        // 首次断流也必须可见；使用错误生命周期事件，不添加额外横幅或重发轮次。
        sess.notify_stream_error(
            turn_context,
            format!("Reconnecting... {retry_count}/{max_retries}"),
            err,
        )
        .await;
        // Use one clock sample so local backoff telemetry retains the selected delay.
        let now = Instant::now();
        let retry_at = retry_after.map(RetryAfter::deadline).unwrap_or(now + delay);
        let delay = retry_at.saturating_duration_since(now);
        codex_client::record_retry!(retry_count, delay, operation);
        tokio::time::sleep_until(retry_at).await;
        return Ok(());
    }

    record_exhausted_retry(sess, turn_context, retry_after);
    Err(err)
}

/// 所有预算耗尽路径都保留服务端建议，避免上层恢复提前重发。
fn record_exhausted_retry(sess: &Session, turn: &TurnContext, advice: Option<RetryAfter>) {
    sess.services
        .thread_extension_data
        .insert(ExhaustedResponseRetry {
            turn_id: turn.sub_id.clone(),
            retry_at: advice.map(RetryAfter::deadline),
        });
}

/// 内容过滤的上下文提示属于历史写入，在重试判定之前保持既有顺序。
async fn record_content_filter_guidance(sess: &Session, step_context: &StepContext) {
    let model_info = &step_context.settings.model_info;
    let guidance = ContentFilterGuidance {
        text: codex_prompts::ResolvedModelMessages::from_model(model_info)
            .content_filter_guidance()
            .to_string(),
    };
    sess.record_conversation_items(
        &step_context.turn,
        model_info,
        &[ContextualUserFragment::into(guidance)],
    )
    .await;
}

fn log_retry(
    request: ResponsesStreamRequest,
    turn_context: &TurnContext,
    err: &CodexErr,
    retries: u64,
    max_retries: u64,
    delay: Duration,
) {
    match request {
        ResponsesStreamRequest::Sampling => {
            warn!(
                turn_id = %turn_context.sub_id,
                retries,
                max_retries,
                sampling_error = %err,
                "stream disconnected - retrying sampling request ({retries}/{max_retries} in {delay:?})...",
            );
        }
        ResponsesStreamRequest::RemoteCompactionV2 => {
            warn!(
                turn_id = %turn_context.sub_id,
                retries,
                max_retries,
                compact_error = %err,
                "remote compaction v2 stream failed; retrying request after delay"
            );
        }
    }
}

#[cfg(test)]
#[path = "responses_retry_tests.rs"]
mod tests;
