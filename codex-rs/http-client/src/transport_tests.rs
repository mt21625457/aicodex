use super::*;
use crate::EncodedJsonBody;
use crate::RequestCompression;
use bytes::Bytes;
use pretty_assertions::assert_eq;
use serde_json::json;

fn request_with_body(body: RequestBody) -> Request {
    Request {
        method: Method::POST,
        url: "https://example.com/v1/responses".to_string(),
        headers: HeaderMap::new(),
        body: Some(body),
        compression: RequestCompression::None,
        timeout: None,
    }
}

#[test]
fn request_body_trace_bounds_json_preview() {
    let secret_suffix = "secret-json-payload";
    let request = request_with_body(RequestBody::Json(json!({
        "payload": format!("{}{}", "a".repeat(TRACE_BODY_PREVIEW_BYTES * 2), secret_suffix),
    })));

    let trace = request_body_for_trace(&request);

    assert!(trace.contains("body_kind=json"));
    assert!(trace.contains("truncated=true"));
    assert!(trace.contains("preview_bytes=2048"));
    assert!(!trace.contains(secret_suffix));
    assert!(trace.len() < TRACE_BODY_PREVIEW_BYTES + 256);
}

#[test]
fn request_body_trace_bounds_encoded_json_preview() {
    let secret_suffix = "secret-encoded-payload";
    let body = EncodedJsonBody::encode(&json!({
        "payload": format!("{}{}", "b".repeat(TRACE_BODY_PREVIEW_BYTES * 2), secret_suffix),
    }))
    .expect("encode json body");
    let original_bytes = body.as_bytes().len();
    let request = request_with_body(RequestBody::EncodedJson(body));

    let trace = request_body_for_trace(&request);

    assert!(trace.contains("body_kind=encoded_json"));
    assert!(trace.contains(&format!("original_bytes={original_bytes}")));
    assert!(trace.contains("truncated=true"));
    assert!(!trace.contains(secret_suffix));
    assert!(trace.len() < TRACE_BODY_PREVIEW_BYTES + 256);
}

#[test]
fn request_body_trace_keeps_small_json_preview() {
    let request = request_with_body(RequestBody::Json(json!({
        "model": "gpt-test",
    })));

    let trace = request_body_for_trace(&request);

    assert_eq!(
        trace,
        "body_kind=json original_bytes=20 preview_bytes=20 truncated=false preview=\"{\\\"model\\\":\\\"gpt-test\\\"}\""
    );
}

#[test]
fn request_body_trace_does_not_split_trailing_utf8_char() {
    let secret_suffix = "secret-raw-payload";
    let mut bytes = vec![b'a'; TRACE_BODY_PREVIEW_BYTES - 1];
    bytes.extend_from_slice("\u{1f600}".as_bytes());
    bytes.extend_from_slice(secret_suffix.as_bytes());
    let request = request_with_body(RequestBody::Raw(Bytes::from(bytes)));

    let trace = request_body_for_trace(&request);

    assert!(trace.contains("body_kind=raw"));
    assert!(trace.contains("preview_bytes=2047"));
    assert!(trace.contains("truncated=true"));
    assert!(!trace.contains('\u{fffd}'));
    assert!(!trace.contains('\u{1f600}'));
    assert!(!trace.contains(secret_suffix));
}
