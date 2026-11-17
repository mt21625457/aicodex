//! 压缩请求的独立等待预算：覆盖建连、流读取和重试，不改变普通对话超时。
//! 仅观察事件种类与时间，不记录摘要、凭据或响应标识；取消时随请求一起释放。

use crate::client_common::ResponseEvent;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result;
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;

#[path = "compaction_budget.rs"]
mod budget;
#[path = "compaction_progress.rs"]
mod progress;
use budget::HARD_LIMIT;
use budget::IDLE_LIMIT;
use budget::SOFT_LIMIT;
const REPORT_INTERVAL: Duration = Duration::from_secs(30);

/// 一次压缩共享一个预算，重连和元数据事件均不能无限延长等待。
pub(crate) struct CompactionWatchdog {
    started: Instant,
    last_progress: Instant,
    first_event: Option<Instant>,
    events: u64,
    attempt: u32,
    attempt_progress: bool,
    last_error_category: Option<&'static str>,
}

impl CompactionWatchdog {
    pub(crate) fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_progress: now,
            first_event: None,
            events: 0,
            attempt: 0,
            attempt_progress: false,
            last_error_category: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn almost_expired() -> Self {
        let mut watchdog = Self::new();
        watchdog.last_progress -= IDLE_LIMIT - Duration::from_millis(200);
        watchdog
    }

    /// 等待只包围网络阶段；成功后写入历史的事务不会被此计时器中途取消。
    pub(crate) async fn wait<T>(
        &self,
        stage: &'static str,
        future: impl Future<Output = T>,
    ) -> Result<T> {
        let deadline = budget::deadline(self.started, self.last_progress, self.attempt_progress);
        tokio::pin!(future);
        let mut reports =
            tokio::time::interval_at(Instant::now() + REPORT_INTERVAL, REPORT_INTERVAL);
        loop {
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(deadline) => {
                    // 按最先到期的预算分类；任务调度延迟可能让两个截止都已过期。
                    let kind = if deadline == self.last_progress + IDLE_LIMIT { "idle" } else { "total" };
                    return Err(self.timeout(kind, stage));
                }
                value = &mut future => return Ok(value),
                _ = reports.tick() => {
                    tracing::info!(operation = "context.compact", stage,
                        elapsed_ms = self.started.elapsed().as_millis() as u64,
                        idle_ms = self.last_progress.elapsed().as_millis() as u64,
                        first_event_ms = self.first_event.map(|t| t.duration_since(self.started).as_millis() as u64),
                        events = self.events, attempt = self.attempt,
                        remaining_ms = HARD_LIMIT.saturating_sub(self.started.elapsed()).as_millis() as u64,
                        last_error_category = self.last_error_category, "compaction request is still pending");
                }
            }
        }
    }

    /// 每个真实出站请求开始一次attempt；失败摘要从未写入历史。
    pub(crate) fn begin_attempt(&mut self) -> Result<()> {
        if self.attempt > 0 {
            self.admit_retry(Duration::ZERO)?;
        }
        self.attempt += 1;
        self.attempt_progress = false;
        self.last_progress = Instant::now();
        Ok(())
    }

    /// 退避和完整idle窗口都必须装进剩余软预算，禁止宽限期再次重放大请求。
    pub(crate) fn admit_retry(&self, backoff: Duration) -> Result<()> {
        if !budget::admits_retry(self.started, backoff) {
            return Err(self.timeout("retry_budget", "retry_admission"));
        }
        Ok(())
    }

    /// 只保留受控错误类别，不记录可能带服务端响应或凭据的错误正文。
    pub(crate) fn record_error(&mut self, error: &CodexErr) {
        use codex_protocol::error::CodexErrorDetails;
        self.last_error_category = Some(match error.details() {
            CodexErrorDetails::Stream(_) | CodexErrorDetails::ResponseStreamFailed(_) => "stream",
            CodexErrorDetails::ConnectionFailed(_) => "connection",
            CodexErrorDetails::RequestTimeout => "request_timeout",
            CodexErrorDetails::ContextWindowExceeded => "context_limit",
            _ => "other",
        });
        tracing::warn!(
            operation = "context.compact",
            stage = "attempt_failed",
            attempt = self.attempt,
            category = self.last_error_category,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            remaining_soft_ms = SOFT_LIMIT
                .saturating_sub(self.started.elapsed())
                .as_millis() as u64,
            "compaction attempt failed"
        );
    }

    fn timeout(&self, kind: &'static str, stage: &'static str) -> CodexErr {
        tracing::warn!(
            code = "compaction.request_timeout",
            operation = "context.compact",
            timeout_kind = kind,
            stage,
            attempt = self.attempt,
            elapsed_ms = self.started.elapsed().as_millis() as u64,
            idle_ms = self.last_progress.elapsed().as_millis() as u64,
            last_error_category = self.last_error_category,
            events = self.events,
            "compaction request exceeded its waiting budget"
        );
        use codex_protocol::compaction_error::CompactionErrorContext;
        use codex_protocol::compaction_error::CompactionTimeoutKind;
        let kind = match kind {
            "idle" => CompactionTimeoutKind::Idle,
            "retry_budget" => CompactionTimeoutKind::RetryBudget,
            _ => CompactionTimeoutKind::Total,
        };
        CodexErr::CompactionTimeout.with_compaction_context(CompactionErrorContext {
            kind,
            attempt: Some(self.attempt),
            stage: Some(stage.to_owned()),
            elapsed_ms: Some(self.started.elapsed().as_millis() as u64),
            idle_ms: Some(self.last_progress.elapsed().as_millis() as u64),
            remaining_ms: Some(
                HARD_LIMIT
                    .saturating_sub(self.started.elapsed())
                    .as_millis() as u64,
            ),
            last_error_category: self.last_error_category.map(str::to_owned),
        })
    }

    /// 元数据、空增量及连接心跳不能冒充摘要生成进展。
    pub(crate) fn observe(&mut self, event: &ResponseEvent) {
        let now = Instant::now();
        self.events += 1;
        if self.first_event.is_none() {
            self.first_event = Some(now);
            tracing::info!(
                operation = "context.compact",
                first_event_ms = now.duration_since(self.started).as_millis() as u64,
                "compaction received its first event"
            );
        }
        let meaningful = match event {
            ResponseEvent::OutputTextDelta(delta)
            | ResponseEvent::ReasoningSummaryDelta { delta, .. }
            | ResponseEvent::ReasoningContentDelta { delta, .. }
            | ResponseEvent::ToolCallInputDelta { delta, .. } => !delta.is_empty(),
            // item开始/summary结束标记不证明产生内容；完成的输出项和完成事件才推进。
            ResponseEvent::OutputItemDone(item) => progress::has_content(item),
            ResponseEvent::Completed { .. } => true,
            _ => false,
        };
        if meaningful {
            self.last_progress = now;
            self.attempt_progress = true;
        }
        if matches!(event, ResponseEvent::Completed { .. }) {
            tracing::info!(
                operation = "context.compact",
                events = self.events,
                elapsed_ms = now.duration_since(self.started).as_millis() as u64,
                "compaction response completed"
            );
        }
    }
}

#[cfg(test)]
#[path = "compaction_watchdog_tests.rs"]
mod tests;
