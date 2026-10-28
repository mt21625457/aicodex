use super::*;
use std::sync::{
    Arc,
    atomic::{AtomicUsize, Ordering},
};

#[tokio::test(start_paused = true)]
async fn stalled_refresh_does_not_block_stream_and_duplicates_are_coalesced() {
    let refresh = ModelsRefreshBackground::default();
    let calls = Arc::new(AtomicUsize::new(0));
    for _ in 0..3 {
        let calls = Arc::clone(&calls);
        refresh.schedule(async move {
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
            .await
            .task
            .as_ref()
            .unwrap()
            .is_finished()
    );
    let calls2 = Arc::clone(&calls);
    refresh.schedule(async move {
        calls2.fetch_add(1, Ordering::SeqCst);
    });
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    tokio::time::advance(COOLDOWN).await;
    let calls2 = Arc::clone(&calls);
    refresh.schedule(async move {
        calls2.fetch_add(1, Ordering::SeqCst);
    });
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[tokio::test(start_paused = true)]
async fn releasing_thread_cancels_its_background_refresh() {
    let refresh = ModelsRefreshBackground::default();
    let (send, receive) = tokio::sync::oneshot::channel::<()>();
    refresh.schedule(async move {
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
    refresh.schedule(async move {
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
    refresh.schedule(async move {
        let _ = send.send(());
    });
    tokio::task::yield_now().await;
    assert_eq!(
        receive.try_recv(),
        Err(tokio::sync::oneshot::error::TryRecvError::Closed)
    );
}
