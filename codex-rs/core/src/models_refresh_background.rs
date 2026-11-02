//! Thread-owned, serial catalog refreshes with bounded latest-version coalescing.

use futures::FutureExt;
use futures::future::BoxFuture;
use futures::future::Shared;
use std::collections::VecDeque;
use std::future::Future;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tokio::task::JoinHandle;
use tokio::time::Instant;
use tracing::Instrument;

const COOLDOWN: Duration = Duration::from_secs(60);
const REFRESH_LIMIT: Duration = Duration::from_secs(10);
const RECENT_KEYS_LIMIT: usize = 16;

#[derive(Clone, PartialEq, Eq)]
pub(crate) struct RefreshKey {
    pub(crate) catalog: usize,
    pub(crate) auth_generation: Option<u64>,
    pub(crate) etag: String,
}

struct PendingRefresh {
    key: RefreshKey,
    future: BoxFuture<'static, ()>,
}

#[derive(Default)]
pub(crate) struct ModelsRefreshBackground {
    state: Arc<Mutex<RefreshState>>,
}

#[derive(Default)]
struct RefreshState {
    task: Option<JoinHandle<()>>,
    shutdown_completion: Option<Shared<BoxFuture<'static, ()>>>,
    recent: VecDeque<(RefreshKey, Instant)>,
    pending: Option<PendingRefresh>,
    running: bool,
    closed: bool,
}

impl RefreshState {
    fn record_started(&mut self, key: RefreshKey) {
        self.recent
            .retain(|(previous, started)| previous != &key && started.elapsed() < COOLDOWN);
        self.recent.push_back((key, Instant::now()));
        while self.recent.len() > RECENT_KEYS_LIMIT {
            self.recent.pop_front();
        }
    }
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
        let completion = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            state.closed = true;
            state.pending = None;
            if state.shutdown_completion.is_none() {
                state.shutdown_completion = state.task.take().map(|task| {
                    task.abort();
                    async move {
                        let _ = task.await;
                    }
                    .boxed()
                    .shared()
                });
            }
            // State owns the join even if a caller is cancelled. No lock crosses the await.
            state.shutdown_completion.clone()
        };
        if let Some(completion) = completion {
            completion.await;
        }
    }

    /// Admit synchronously without network work. A newer version replaces only pending work.
    pub(crate) fn schedule(
        &self,
        key: RefreshKey,
        future: impl Future<Output = ()> + Send + 'static,
    ) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if state.closed {
            return;
        }
        state
            .recent
            .retain(|(_, started)| started.elapsed() < COOLDOWN);
        let worker_running =
            state.running && state.task.as_ref().is_some_and(|task| !task.is_finished());
        if state.recent.iter().any(|(previous, _)| previous == &key)
            || (worker_running
                && state
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.key == key))
        {
            return;
        }
        let next = PendingRefresh {
            key,
            future: future.in_current_span().boxed(),
        };
        if worker_running {
            state.pending = Some(next);
            return;
        }
        if let Some(task) = state.task.take()
            && let Some(Err(error)) = task.now_or_never()
        {
            tracing::error!(code = "model_catalog.background_refresh_failed",
                operation = "models.refresh", %error, "background refresh task failed");
        }
        // A failed worker can leave a pending version; this newly admitted version is newer.
        state.pending = None;
        state.running = true;
        state.record_started(next.key.clone());
        let weak_state = Arc::downgrade(&self.state);
        state.task = Some(tokio::spawn(
            async move {
                let mut next = next;
                loop {
                    if tokio::time::timeout(REFRESH_LIMIT, next.future)
                        .await
                        .is_err()
                    {
                        tracing::warn!(
                            code = "model_catalog.background_refresh_timeout",
                            operation = "models.refresh",
                            "background model catalog refresh timed out"
                        );
                    }
                    // Never keep state alive while awaiting the network: dropping the owner must abort.
                    let Some(shared) = weak_state.upgrade() else {
                        return;
                    };
                    let pending = {
                        let mut state = shared
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner);
                        if state.closed {
                            return;
                        }
                        match state.pending.take() {
                            Some(pending) => {
                                state.record_started(pending.key.clone());
                                Some(pending)
                            }
                            None => {
                                state.running = false;
                                None
                            }
                        }
                    };
                    drop(shared);
                    let Some(pending) = pending else {
                        return;
                    };
                    next = pending;
                }
            }
            .in_current_span(),
        ));
    }
}

#[cfg(test)]
#[path = "models_refresh_background_tests.rs"]
mod tests;
