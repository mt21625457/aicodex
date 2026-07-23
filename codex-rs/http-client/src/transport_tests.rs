use super::*;
use crate::EncodedJsonBody;
use crate::RequestCompression;
use bytes::Bytes;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::io::Write;
use std::sync::Arc;
use std::sync::Mutex;
use std::time::Duration;
use tracing_subscriber::Layer;
use tracing_subscriber::layer::SubscriberExt;

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

#[tokio::test]
async fn enabled_request_logging_emits_transport_url_and_body() {
    let logs = capture_transport_logs(HttpClient::new(test_reqwest_client())).await;

    assert!(logs.contains("log capture sentinel"));
    assert!(logs.contains("url-secret"));
    assert!(logs.contains("body-secret"));
}

#[tokio::test]
async fn disabled_request_logging_suppresses_transport_url_and_body() {
    let logs = capture_transport_logs(HttpClient::new_without_request_logging(
        test_reqwest_client(),
    ))
    .await;

    assert!(logs.contains("log capture sentinel"));
    assert!(!logs.contains("url-secret"));
    assert!(!logs.contains("body-secret"));
}

fn test_reqwest_client() -> reqwest::Client {
    reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("HTTP client should build")
}

async fn capture_transport_logs(client: HttpClient) -> String {
    let unavailable_server =
        std::net::TcpListener::bind(("127.0.0.1", 0)).expect("server port should bind");
    let server_addr = unavailable_server
        .local_addr()
        .expect("server listener should have an address");
    drop(unavailable_server);
    let transport = ReqwestTransport::from_http_client(client);
    let log_buffer = Arc::new(Mutex::new(Vec::new()));
    let writer_buffer = Arc::clone(&log_buffer);
    let subscriber = tracing_subscriber::registry().with(
        tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(move || TestLogWriter(Arc::clone(&writer_buffer)))
            .with_filter(
                tracing_subscriber::filter::Targets::new()
                    .with_target("codex_http_client::transport", tracing::Level::TRACE),
            ),
    );
    let _guard = tracing::subscriber::set_default(subscriber);
    tracing::trace!(target: "codex_http_client::transport", "log capture sentinel");
    let mut request = Request::new(
        Method::POST,
        format!("http://{server_addr}/request?token=url-secret"),
    )
    .with_json(&json!({"token": "body-secret"}));
    request.timeout = Some(Duration::from_secs(1));

    let _ = transport.execute(request).await;

    String::from_utf8(
        log_buffer
            .lock()
            .expect("log buffer should not be poisoned")
            .clone(),
    )
    .expect("captured logs should be UTF-8")
}

#[derive(Clone)]
struct TestLogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for TestLogWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0
            .lock()
            .map_err(|_| std::io::Error::other("log buffer should not be poisoned"))?
            .extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}
