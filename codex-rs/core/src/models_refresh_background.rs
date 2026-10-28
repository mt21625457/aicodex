//! 目录刷新由线程拥有，不能阻塞当前响应流；同线程最多一个任务，失败也有冷却。

use futures::FutureExt;
use std::future::Future;
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::Instrument;

const COOLDOWN: Duration = Duration::from_secs(60);
const REFRESH_LIMIT: Duration = Duration::from_secs(10);

#[derive(Default)]
pub(crate) struct ModelsRefreshBackground {
    state: Mutex<RefreshState>,
}

#[derive(Default)]
struct RefreshState {
    task: Option<JoinHandle<()>>,
    last_started: Option<Instant>,
    closed: bool,
}

impl Drop for RefreshState {
    fn drop(&mut self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }
}

impl ModelsRefreshBackground {
    /// Stop admission and wait until the refresh future has released its resources.
    pub(crate) async fn shutdown(&self) {
        let mut state = self.state.lock().await;
        state.closed = true;
        // Keep the handle in state while awaiting so cancellation of this shutdown
        // cannot detach it, and concurrent shutdown callers wait for the same task.
        if let Some(task) = state.task.as_mut() {
            task.abort();
            let _ = task.await;
        }
        state.task = None;
    }

    /// 锁竞争直接合并请求；这里只调度，不等待网络，也不跨刷新持有状态锁。
    pub(crate) fn schedule(&self, future: impl Future<Output = ()> + Send + 'static) {
        let Ok(mut state) = self.state.try_lock() else {
            return;
        };
        if state.closed
            || state.task.as_ref().is_some_and(|task| !task.is_finished())
            || state
                .last_started
                .is_some_and(|started| started.elapsed() < COOLDOWN)
        {
            return;
        }
        if let Some(task) = state.task.take()
            && let Some(Err(error)) = task.now_or_never()
        {
            tracing::error!(code = "model_catalog.background_refresh_failed",
                operation = "models.refresh", %error, "background refresh task failed");
        }
        state.last_started = Some(Instant::now());
        state.task = Some(tokio::spawn(
            async move {
                if tokio::time::timeout(REFRESH_LIMIT, future).await.is_err() {
                    tracing::warn!(
                        code = "model_catalog.background_refresh_timeout",
                        operation = "models.refresh",
                        "background model catalog refresh timed out"
                    );
                }
            }
            .in_current_span(),
        ));
    }
}

#[cfg(test)]
#[path = "models_refresh_background_tests.rs"]
mod tests;
