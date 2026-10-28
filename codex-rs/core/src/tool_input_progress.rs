//! 观察模型流中的工具参数生成，按秒合并进度；不修改、持久化或执行参数。
use codex_api::ResponseEvent;
use codex_protocol::models::ResponseItem;
use codex_protocol::tool_input_progress::ToolInputProgressEvent;
use tokio::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct ToolInputProgress {
    active: Option<ActiveInput>,
}

struct ActiveInput {
    item_id: String,
    call_id: String,
    bytes: u64,
    last_emitted: Instant,
}

impl ActiveInput {
    fn snapshot(&self, finished: bool) -> ToolInputProgressEvent {
        ToolInputProgressEvent {
            item_id: self.item_id.clone(),
            input_bytes: self.bytes,
            finished,
        }
    }
}

/// 返回协议提供的身份；只有参数计数出站，不发送工具名或参数正文。
fn tool_identity(item: &ResponseItem) -> Option<(String, String)> {
    match item {
        ResponseItem::CustomToolCall { id, call_id, .. }
        | ResponseItem::FunctionCall { id, call_id, .. } => Some((
            id.as_ref()
                .map_or_else(|| call_id.clone(), ToString::to_string),
            call_id.clone(),
        )),
        _ => None,
    }
}

impl ToolInputProgress {
    /// 顺序消费同一采样流；较早并行调用的增量或完成不能清除最新调用的进度。
    pub(crate) fn observe(&mut self, event: &ResponseEvent) -> Option<ToolInputProgressEvent> {
        match event {
            ResponseEvent::OutputItemAdded(item) => {
                let (item_id, call_id) = tool_identity(item)?;
                let active = ActiveInput {
                    item_id,
                    call_id,
                    bytes: 0,
                    last_emitted: Instant::now(),
                };
                let progress = active.snapshot(false);
                self.active = Some(active);
                Some(progress)
            }
            ResponseEvent::ToolCallInputDelta {
                item_id,
                call_id,
                delta,
            } => {
                let active = self.active.as_mut()?;
                if delta.is_empty()
                    || (active.item_id != *item_id
                        && call_id.as_deref() != Some(active.call_id.as_str())
                        && active.call_id != *item_id)
                {
                    return None;
                }
                active.bytes = active.bytes.saturating_add(delta.len() as u64);
                if active.last_emitted.elapsed() < Duration::from_secs(1) {
                    return None;
                }
                active.last_emitted = Instant::now();
                Some(active.snapshot(false))
            }
            ResponseEvent::OutputItemDone(item) => {
                let (item_id, call_id) = tool_identity(item)?;
                let active = self.active.as_ref()?;
                if active.item_id != item_id && active.call_id != call_id {
                    return None;
                }
                self.finish()
            }
            _ => None,
        }
    }

    /// 正常完成、错误与抢占共用清理；finished 仅结束生成状态，不冒充执行结果。
    pub(crate) fn finish(&mut self) -> Option<ToolInputProgressEvent> {
        self.active.take().map(|active| active.snapshot(true))
    }
}

#[cfg(test)]
#[path = "tool_input_progress_tests.rs"]
mod tests;
