//! 压缩请求的独立等待预算：覆盖建连、流读取和重试，不改变普通对话超时。
//! 仅观察事件种类与时间，不记录摘要、凭据或响应标识；取消时随请求一起释放。

use crate::client_common::ResponseEvent;
use codex_protocol::error::{CodexErr, Result};
use std::future::Future;
use std::time::Duration;
use tokio::time::Instant;

const IDLE_LIMIT: Duration = Duration::from_secs(120);
const TOTAL_LIMIT: Duration = Duration::from_secs(300);
const REPORT_INTERVAL: Duration = Duration::from_secs(30);

/// 一次压缩共享一个预算，重连和元数据事件均不能无限延长等待。
pub(crate) struct CompactionWatchdog {
    started: Instant,
    last_progress: Instant,
    first_event: Option<Instant>,
    events: u64,
}

impl CompactionWatchdog {
    pub(crate) fn new() -> Self {
        let now = Instant::now();
        Self {
            started: now,
            last_progress: now,
            first_event: None,
            events: 0,
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
        let deadline = (self.started + TOTAL_LIMIT).min(self.last_progress + IDLE_LIMIT);
        tokio::pin!(future);
        let mut reports =
            tokio::time::interval_at(Instant::now() + REPORT_INTERVAL, REPORT_INTERVAL);
        loop {
            tokio::select! {
                biased;
                _ = tokio::time::sleep_until(deadline) => {
                    tracing::warn!(code = "compaction.request_timeout", operation = "context.compact",
                        stage, elapsed_ms = self.started.elapsed().as_millis() as u64,
                        idle_ms = self.last_progress.elapsed().as_millis() as u64,
                        events = self.events, "compaction request exceeded its waiting budget");
                    return Err(CodexErr::CompactionTimeout);
                }
                value = &mut future => return Ok(value),
                _ = reports.tick() => {
                    tracing::info!(operation = "context.compact", stage,
                        elapsed_ms = self.started.elapsed().as_millis() as u64,
                        idle_ms = self.last_progress.elapsed().as_millis() as u64,
                        first_event_ms = self.first_event.map(|t| t.duration_since(self.started).as_millis() as u64),
                        events = self.events, "compaction request is still pending");
                }
            }
        }
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
            ResponseEvent::OutputItemAdded(_)
            | ResponseEvent::OutputItemDone(_)
            | ResponseEvent::ReasoningSummaryDone { .. }
            | ResponseEvent::Completed { .. } => true,
            _ => false,
        };
        if meaningful {
            self.last_progress = now;
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
