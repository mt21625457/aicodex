//! Route-aware egress planning. Never mutate durable history or screenshot geometry.
use crate::error::ApiError;
use crate::provider::Provider;
use codex_http_client::EncodedJsonBody;
use codex_utils_image::TransportImageEncoding;
use serde_json::Value;

const MIB: usize = 1024 * 1024;
static WORKERS: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(2);

#[derive(Clone, Copy, Debug)]
pub(crate) struct RequestBudget {
    route_bytes: usize,
}

impl RequestBudget {
    pub(crate) fn for_provider(provider: &Provider) -> Self {
        // Endpoint identity, not display name, establishes direct API defaults.
        // Other routes use a conservative CLIENT default, not a claimed server limit.
        let direct_openai = url::Url::parse(&provider.base_url).is_ok_and(|url| {
            url.scheme() == "https"
                && url.host_str() == Some("api.openai.com")
                && url.path().trim_end_matches('/') == "/v1"
                && url.port_or_known_default() == Some(443)
        });
        Self {
            route_bytes: provider.request_body_max_bytes.unwrap_or(if direct_openai {
                480 * MIB
            } else {
                40 * MIB
            }),
        }
    }

    fn limit(self, model: &str) -> usize {
        let normalized = model.trim().to_ascii_lowercase();
        let slug = normalized.rsplit([':', '/']).next().unwrap_or_default();
        if slug.starts_with("deepseek-") {
            self.route_bytes.min(40 * MIB)
        } else {
            self.route_bytes
        }
    }
}

pub(crate) async fn prepare(
    body: Value,
    budget: RequestBudget,
) -> Result<EncodedJsonBody, ApiError> {
    let permit = WORKERS
        .acquire()
        .await
        .map_err(|_| invalid("Image preparation unavailable"))?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        prepare_sync(body, budget)
    })
    .await
    .map_err(|_| invalid("Image preparation worker failed"))?
}

struct ImageSlot {
    item: usize,
    field: &'static str,
    content: usize,
    source: String,
    original: bool,
}

fn prepare_sync(mut body: Value, budget: RequestBudget) -> Result<EncodedJsonBody, ApiError> {
    let max_request_bytes = budget.limit(body["model"].as_str().unwrap_or_default());
    let encoded = encode(&body)?;
    let before = encoded.as_bytes().len();
    if before <= max_request_bytes {
        tracing::debug!(
            stage = "provider_request_budget",
            request_bytes = before,
            max_request_bytes,
            "Request fits without image re-encoding"
        );
        return Ok(encoded);
    }
    drop(encoded);
    let mut slots = Vec::new();
    // Only protocol image slots; never interpret quoted content, schemas or metadata.
    for (item_index, item) in body["input"].as_array().into_iter().flatten().enumerate() {
        let field = match item["type"].as_str() {
            Some("message") | None => "content",
            Some("function_call_output" | "custom_tool_call_output") => "output",
            _ => continue,
        };
        for (content_index, content) in item[field].as_array().into_iter().flatten().enumerate() {
            if content["type"] != "input_image" {
                continue;
            }
            let Some(url) = content["image_url"]
                .as_str()
                .filter(|url| url.starts_with("data:"))
            else {
                continue;
            };
            slots.push(ImageSlot {
                item: item_index,
                field,
                content: content_index,
                source: url.to_owned(),
                original: content["detail"] == "original",
            });
        }
    }
    // Spend work on the largest representations first, retaining order in the request.
    slots.sort_by_key(|slot| std::cmp::Reverse(slot.source.len()));
    let mut request_bytes = before;
    for encoding in [
        TransportImageEncoding::Lossless,
        TransportImageEncoding::Jpeg85,
        TransportImageEncoding::Jpeg75,
        TransportImageEncoding::Jpeg65,
    ] {
        for slot in &slots {
            if slot.original && encoding != TransportImageEncoding::Lossless {
                continue;
            }
            let candidate =
                codex_utils_image::optimize_data_url_for_transport(&slot.source, encoding)
                    .map_err(|reason| invalid(format!("Image preparation failed: {reason}")))?;
            let current = &mut body["input"][slot.item][slot.field][slot.content]["image_url"];
            // Compare actual JSON string sizes, including escaping and base64 expansion.
            let current_bytes = serde_json::to_vec(&*current)
                .map_err(|_| invalid("Cannot encode image"))?
                .len();
            let candidate_bytes = serde_json::to_vec(&candidate)
                .map_err(|_| invalid("Cannot encode image"))?
                .len();
            if candidate_bytes < current_bytes {
                request_bytes -= current_bytes - candidate_bytes;
                *current = Value::String(candidate);
            }
            if request_bytes <= max_request_bytes {
                let prepared = encode(&body)?;
                if prepared.as_bytes().len() <= max_request_bytes {
                    tracing::debug!(
                        stage = "provider_request_prepared",
                        before,
                        request_bytes = prepared.as_bytes().len(),
                        max_request_bytes,
                        image_count = slots.len(),
                        "Prepared bounded request copy"
                    );
                    return Ok(prepared);
                }
            }
        }
    }
    Err(invalid(format!(
        "Request body is {request_bytes} bytes after image preparation; route/model client budget is {max_request_bytes} bytes. Original-detail images remain lossless and screenshot dimensions are preserved. Reduce attachments or explicitly compact the conversation. The request was not sent."
    )))
}

fn encode(body: &Value) -> Result<EncodedJsonBody, ApiError> {
    EncodedJsonBody::encode(body).map_err(|_| invalid("Cannot encode Responses request"))
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::InvalidRequest {
        message: message.into(),
    }
}

#[cfg(test)]
#[path = "request_budget_tests.rs"]
mod tests;
