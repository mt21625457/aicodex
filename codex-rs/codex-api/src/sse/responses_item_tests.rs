//! 已知输出事件损坏必须可见；共享解码器同时保护 HTTP SSE 与 WebSocket。
use super::responses::ResponsesStreamEvent;
use super::responses::process_responses_event;
use crate::error::ApiError;
use serde_json::json;

#[test]
fn malformed_output_items_fail_without_exposing_payload() {
    for kind in ["response.output_item.added", "response.output_item.done"] {
        for item in [
            serde_json::Value::Null,
            json!({"type":"function_call", "name":"private-content", "arguments":42}),
        ] {
            let event: ResponsesStreamEvent =
                serde_json::from_value(json!({"type":kind, "item":item})).unwrap();
            let error = process_responses_event(event).unwrap_err().into_api_error();
            assert!(matches!(&error, ApiError::MalformedResponse { .. }));
            assert!(error.to_string().contains(kind));
            assert!(!error.to_string().contains("private-content"));
        }
    }
}

#[test]
fn unknown_extension_items_remain_forward_compatible() {
    for kind in ["response.output_item.added", "response.output_item.done"] {
        let event = serde_json::from_value(
            json!({"type":kind, "item":{"type":"future_extension", "extra":true}}),
        )
        .unwrap();
        assert!(process_responses_event(event).unwrap().is_some());
    }
}

#[tokio::test]
async fn corrupt_item_stops_sse_before_successful_completion() {
    use bytes::Bytes;
    use futures::stream;
    use std::time::Duration;
    use tokio::sync::mpsc;
    let body = concat!(
        "data: {\"type\":\"response.output_item.done\",\"item\":{\"type\":\"function_call\",\"arguments\":42}}\n\n",
        "data: {\"type\":\"response.completed\",\"response\":{\"id\":\"synthetic\"}}\n\n"
    );
    let stream = stream::iter([Ok(Bytes::from_static(body.as_bytes()))]);
    let (sender, mut receiver) = mpsc::channel(16);
    super::responses::process_sse(Box::pin(stream), sender, Duration::from_secs(1), None).await;
    assert!(receiver.recv().await.unwrap().is_err());
    assert!(
        receiver.recv().await.is_none(),
        "损坏输出之后不能继续发布成功完成"
    );
}
