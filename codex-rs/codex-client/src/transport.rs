use crate::default_client::CodexHttpClient;
use crate::default_client::CodexRequestBuilder;
use crate::error::TransportError;
use crate::request::Request;
use crate::request::RequestBody;
use crate::request::Response;
use bytes::Bytes;
use futures::StreamExt;
use futures::stream::BoxStream;
use http::HeaderMap;
use http::Method;
use http::StatusCode;
use tracing::Level;
use tracing::enabled;
use tracing::trace;

const TRACE_BODY_PREVIEW_BYTES: usize = 2 * 1024;

pub type ByteStream = BoxStream<'static, Result<Bytes, TransportError>>;

pub struct StreamResponse {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub bytes: ByteStream,
}

pub trait HttpTransport: Send + Sync {
    fn execute(
        &self,
        req: Request,
    ) -> impl std::future::Future<Output = Result<Response, TransportError>> + Send;
    fn stream(
        &self,
        req: Request,
    ) -> impl std::future::Future<Output = Result<StreamResponse, TransportError>> + Send;
}

#[derive(Clone, Debug)]
pub struct ReqwestTransport {
    client: CodexHttpClient,
}

impl ReqwestTransport {
    pub fn new(client: reqwest::Client) -> Self {
        Self {
            client: CodexHttpClient::new(client),
        }
    }

    fn build(&self, req: Request) -> Result<CodexRequestBuilder, TransportError> {
        let prepared = req.prepare_body_for_send().map_err(TransportError::Build)?;

        let Request {
            method,
            url,
            headers: _,
            body: _,
            compression: _,
            timeout,
        } = req;

        let mut builder = self.client.request(
            Method::from_bytes(method.as_str().as_bytes()).unwrap_or(Method::GET),
            &url,
        );

        if let Some(timeout) = timeout {
            builder = builder.timeout(timeout);
        }

        builder = builder.headers(prepared.headers);
        if let Some(body) = prepared.body {
            builder = builder.body(body);
        }
        Ok(builder)
    }

    fn map_error(err: reqwest::Error) -> TransportError {
        if err.is_timeout() {
            TransportError::Timeout
        } else {
            TransportError::Network(err.to_string())
        }
    }
}

fn request_body_for_trace(req: &Request) -> String {
    match req.body.as_ref() {
        Some(RequestBody::Json(body)) => bounded_body_trace("json", body.to_string().as_bytes()),
        Some(RequestBody::EncodedJson(body)) => {
            bounded_body_trace("encoded_json", body.trace_bytes())
        }
        Some(RequestBody::Raw(body)) => bounded_body_trace("raw", body),
        None => bounded_body_trace("none", &[]),
    }
}

fn bounded_body_trace(body_kind: &str, bytes: &[u8]) -> String {
    let preview_bytes = trace_preview_bytes(bytes);
    let preview = String::from_utf8_lossy(&bytes[..preview_bytes]);
    let truncated = bytes.len() > preview_bytes;
    let original_bytes = bytes.len();
    format!(
        "body_kind={body_kind} original_bytes={original_bytes} preview_bytes={preview_bytes} truncated={truncated} preview={preview:?}"
    )
}

fn trace_preview_bytes(bytes: &[u8]) -> usize {
    if bytes.len() <= TRACE_BODY_PREVIEW_BYTES {
        return bytes.len();
    }

    let preview_bytes = TRACE_BODY_PREVIEW_BYTES;
    match std::str::from_utf8(&bytes[..preview_bytes]) {
        Ok(_) => preview_bytes,
        Err(err) if err.error_len().is_none() => err.valid_up_to(),
        Err(_) => preview_bytes,
    }
}

impl HttpTransport for ReqwestTransport {
    async fn execute(&self, req: Request) -> Result<Response, TransportError> {
        if enabled!(Level::TRACE) {
            trace!(
                "{} to {}: {}",
                req.method,
                req.url,
                request_body_for_trace(&req)
            );
        }

        let url = req.url.clone();
        let builder = self.build(req)?;
        let resp = builder.send().await.map_err(Self::map_error)?;
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = resp.bytes().await.map_err(Self::map_error)?;
        if !status.is_success() {
            let body = String::from_utf8(bytes.to_vec()).ok();
            return Err(TransportError::Http {
                status,
                url: Some(url),
                headers: Some(headers),
                body,
            });
        }
        Ok(Response {
            status,
            headers,
            body: bytes,
        })
    }

    async fn stream(&self, req: Request) -> Result<StreamResponse, TransportError> {
        if enabled!(Level::TRACE) {
            trace!(
                "{} to {}: {}",
                req.method,
                req.url,
                request_body_for_trace(&req)
            );
        }

        let url = req.url.clone();
        let builder = self.build(req)?;
        let resp = builder.send().await.map_err(Self::map_error)?;
        let status = resp.status();
        let headers = resp.headers().clone();
        if !status.is_success() {
            let body = resp.text().await.ok();
            return Err(TransportError::Http {
                status,
                url: Some(url),
                headers: Some(headers),
                body,
            });
        }
        let stream = resp
            .bytes_stream()
            .map(|result| result.map_err(Self::map_error));
        Ok(StreamResponse {
            status,
            headers,
            bytes: Box::pin(stream),
        })
    }
}

#[cfg(test)]
#[path = "transport_tests.rs"]
mod tests;
