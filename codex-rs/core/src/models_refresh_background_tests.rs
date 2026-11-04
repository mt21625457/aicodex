use super::*;
use std::sync::Arc;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;

#[tokio::test(start_paused = true)]
async fn stalled_refresh_does_not_block_stream_and_duplicates_are_coalesced() {
    let refresh = ModelsRefreshBackground::default();
    let calls = Arc::new(AtomicUsize::new(0));
    for _ in 0..3 {
        let calls = Arc::clone(&calls);
        refresh.schedule(key("etag-a"), async move {
            calls.fetch_add(1, Ordering::SeqCst);
            std::future::pending::<()>().await;
        });
    }
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    // 调度返回即能消费正文，不需要推进目录请求的超时。
    let stream = futures::stream::iter(["body", "completed"]);
    use futures::StreamExt;
    assert_eq!(stream.collect::<Vec<_>>().await, vec!["body", "completed"]);
    tokio::time::advance(REFRESH_LIMIT).await;
    tokio::task::yield_now().await;
    assert!(
        refresh
            .state
            .lock()
            .unwrap()
            .task
            .as_ref()
            .unwrap()
            .is_finished()
    );
    let calls2 = Arc::clone(&calls);
    refresh.schedule(key("etag-a"), async move {
        calls2.fetch_add(1, Ordering::SeqCst);
    });
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(COOLDOWN).await;
    let calls2 = Arc::clone(&calls);
    refresh.schedule(key("etag-a"), async move {
        calls2.fetch_add(1, Ordering::SeqCst);
    });
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn releasing_thread_cancels_its_background_refresh() {
    let refresh = ModelsRefreshBackground::default();
    let (send, receive) = tokio::sync::oneshot::channel::<()>();
    refresh.schedule(key("etag-a"), async move {
        let _sender = send;
        std::future::pending::<()>().await;
    });
    tokio::task::yield_now().await;
    drop(refresh);
    assert!(receive.await.is_err());
}

#[tokio::test(start_paused = true)]
async fn shutdown_waits_for_refresh_release_and_rejects_later_scheduling() {
    let refresh = ModelsRefreshBackground::default();
    let (started, running) = tokio::sync::oneshot::channel();
    let (send, mut receive) = tokio::sync::oneshot::channel::<()>();
    refresh.schedule(key("etag-a"), async move {
        let _sender = send;
        let _ = started.send(());
        std::future::pending::<()>().await;
    });
    running.await.unwrap();

    tokio::join!(refresh.shutdown(), refresh.shutdown());
    assert_eq!(
        receive.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    );

    // A closed service must stay closed even after its normal cooldown expires.
    tokio::time::advance(COOLDOWN).await;
    let (send, mut receive) = tokio::sync::oneshot::channel::<()>();
    refresh.schedule(key("etag-a"), async move {
        let _ = send.send(());
    });
    tokio::task::yield_now().await;
    assert_eq!(
        receive.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    );
}

#[tokio::test(start_paused = true)]
async fn cancelled_shutdown_keeps_refresh_release_joinable() {
    let refresh = ModelsRefreshBackground::default();
    let (started, running) = tokio::sync::oneshot::channel();
    let (send, mut receive) = tokio::sync::oneshot::channel::<()>();
    refresh.schedule(key("etag-a"), async move {
        let _sender = send;
        let _ = started.send(());
        std::future::pending::<()>().await;
    });
    running.await.unwrap();

    // Poll without yielding to the aborted task so its resources are still live.
    let mut first = Box::pin(refresh.shutdown());
    assert!(futures::poll!(first.as_mut()).is_pending());
    assert!(refresh.state.try_lock().is_ok());
    drop(first);
    assert_eq!(
        receive.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    );

    // A cancelled caller must not consume the only join handle and let this caller
    // report completion before the refresh future releases its resources.
    let mut second = Box::pin(refresh.shutdown());
    assert!(futures::poll!(second.as_mut()).is_pending());
    second.await;
    assert_eq!(
        receive.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    );
}

fn key(etag: &str) -> RefreshKey {
    RefreshKey {
        catalog: 0,
        auth_generation: None,
        etag: etag.to_string(),
    }
}

#[tokio::test(start_paused = true)]
async fn new_etag_and_scope_bypass_previous_key_cooldown() {
    let refresh = ModelsRefreshBackground::default();
    let calls = Arc::new(AtomicUsize::new(0));
    for next_key in [
        key("a"),
        key("b"),
        RefreshKey {
            catalog: 1,
            ..key("b")
        },
        RefreshKey {
            auth_generation: Some(1),
            ..key("b")
        },
    ] {
        let calls = Arc::clone(&calls);
        let (done, received) = tokio::sync::oneshot::channel();
        refresh.schedule(next_key, async move {
            calls.fetch_add(1, Ordering::SeqCst);
            let _ = done.send(());
        });
        received.await.unwrap();
    }
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    let calls_again = Arc::clone(&calls);
    refresh.schedule(key("a"), async move {
        calls_again.fetch_add(1, Ordering::SeqCst);
    });
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 4);
    refresh.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn inflight_refresh_retains_latest_pending_version() {
    let refresh = ModelsRefreshBackground::default();
    let (started, running) = tokio::sync::oneshot::channel();
    let (release, gate) = tokio::sync::oneshot::channel();
    refresh.schedule(key("a"), async move {
        let _ = started.send(());
        let _ = gate.await;
    });
    running.await.unwrap();
    let (discarded, mut discarded_rx) = tokio::sync::oneshot::channel();
    refresh.schedule(key("b"), async move {
        let _ = discarded.send(());
    });
    let (latest, mut latest_rx) = tokio::sync::oneshot::channel();
    refresh.schedule(key("c"), async move {
        let _ = latest.send(());
    });
    assert_eq!(
        discarded_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    );
    assert_eq!(
        latest_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Empty)
    );
    release.send(()).unwrap();
    latest_rx.await.unwrap();
    // Replaced pending B never started and must not have acquired a cooldown.
    let (second_b, received) = tokio::sync::oneshot::channel();
    refresh.schedule(key("b"), async move {
        let _ = second_b.send(());
    });
    received.await.unwrap();
    refresh.shutdown().await;
}

#[tokio::test(start_paused = true)]
async fn shutdown_releases_pending_refresh_without_running_it() {
    let refresh = ModelsRefreshBackground::default();
    let (started, running) = tokio::sync::oneshot::channel();
    let (active, mut active_rx) = tokio::sync::oneshot::channel::<()>();
    refresh.schedule(key("a"), async move {
        let _active = active;
        let _ = started.send(());
        std::future::pending::<()>().await;
    });
    running.await.unwrap();
    let (pending, mut pending_rx) = tokio::sync::oneshot::channel();
    refresh.schedule(key("b"), async move {
        let _ = pending.send(());
    });
    tokio::join!(refresh.shutdown(), refresh.shutdown());
    assert_eq!(
        active_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    );
    assert_eq!(
        pending_rx.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    );
}

#[tokio::test(start_paused = true)]
async fn worker_failure_cannot_replay_stale_pending_after_new_version() {
    // A replacement for the queued key must also recover instead of being deduplicated forever.
    for next_etag in ["b", "c"] {
        let refresh = ModelsRefreshBackground::default();
        let (started, running) = tokio::sync::oneshot::channel();
        let (release, gate) = tokio::sync::oneshot::channel();
        refresh.schedule(key("a"), async move {
            let _ = started.send(());
            let _ = gate.await;
            panic!("synthetic catalog refresh failure");
        });
        running.await.unwrap();
        let (stale, mut stale_rx) = tokio::sync::oneshot::channel();
        refresh.schedule(key("b"), async move {
            let _ = stale.send(());
        });
        release.send(()).unwrap();
        while !refresh
            .state
            .lock()
            .unwrap()
            .task
            .as_ref()
            .unwrap()
            .is_finished()
        {
            tokio::task::yield_now().await;
        }
        let (latest, received) = tokio::sync::oneshot::channel();
        refresh.schedule(key(next_etag), async move {
            let _ = latest.send(());
        });
        received.await.unwrap();
        assert_eq!(
            stale_rx.try_recv(),
            Err(tokio::sync::oneshot::error::TryRecvError::Closed)
        );
        refresh.shutdown().await;
    }
}
