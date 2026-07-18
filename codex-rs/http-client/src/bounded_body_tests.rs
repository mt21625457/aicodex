use super::*;
use futures::stream;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn bounded_collection_accepts_exact_limit() {
    let body = collect_bytes_bounded(
        stream::iter([Ok::<_, io::Error>(Bytes::from_static(b"abc"))]),
        3,
    )
    .await
    .expect("exact limit should succeed");
    assert_eq!(body, b"abc");
}

#[tokio::test]
async fn bounded_collection_rejects_before_collecting_later_chunks() {
    let mut polls = 0;
    let stream = stream::poll_fn(move |_| {
        polls += 1;
        std::task::Poll::Ready(match polls {
            1 => Some(Ok::<_, io::Error>(Bytes::from_static(b"abc"))),
            2 => Some(Ok(Bytes::from_static(b"def"))),
            _ => panic!("bounded collector polled after detecting the oversized body"),
        })
    });
    let error = collect_bytes_bounded(stream, 5)
        .await
        .expect_err("oversized body should fail");
    assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    assert!(error.to_string().contains("5-byte"));
}
