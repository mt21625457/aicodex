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
        if is_deepseek_model(model) {
            self.route_bytes.min(40 * MIB)
        } else {
            self.route_bytes
        }
    }
}

fn is_deepseek_model(model: &str) -> bool {
    let normalized = model.trim().to_ascii_lowercase();
    normalized
        .rsplit([':', '/'])
        .next()
        .is_some_and(|slug| slug.starts_with("deepseek-"))
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
    let model = body["model"].as_str().unwrap_or_default();
    let max_request_bytes = budget.limit(model);
    let deepseek = is_deepseek_model(model);
    let mut encoded = encode(&body)?;
    let before = encoded.as_bytes().len();
    // Leave 20% for the next tool results. This is a request-copy policy only;
    // user attachments and the latest image-bearing tool batch remain available.
    let history_target_bytes = max_request_bytes - max_request_bytes.div_ceil(5);
    if deepseek && before > history_target_bytes {
        let omitted_images = omit_older_tool_images(&mut body, before, history_target_bytes)?;
        if omitted_images > 0 {
            drop(encoded);
            encoded = encode(&body)?;
            tracing::debug!(
                stage = "deepseek_image_history_budget",
                before,
                request_bytes = encoded.as_bytes().len(),
                history_target_bytes,
                omitted_images,
                "Omitted older tool images from request copy"
            );
        }
    }
    let prepared_bytes = encoded.as_bytes().len();
    if prepared_bytes <= max_request_bytes {
        tracing::debug!(
            stage = "provider_request_budget",
            request_bytes = prepared_bytes,
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
    let mut request_bytes = prepared_bytes;
    let mut image_preparation = codex_utils_image::TransportImagePreparationCache::default();
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
            let candidate = image_preparation
                .optimize(&slot.source, encoding)
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

fn is_tool_output(item: &Value) -> bool {
    // Standalone function-call outputs without a call_id are host-delivered turn
    // input, not disposable results of model tool calls (see turn_input admission).
    matches!(
        item["type"].as_str(),
        Some("function_call_output" | "custom_tool_call_output")
    ) && item["call_id"]
        .as_str()
        .is_some_and(|id| !id.trim().is_empty())
}

fn is_tool_call(item: &Value) -> bool {
    matches!(
        item["type"].as_str(),
        Some("function_call" | "custom_tool_call")
    )
}

fn is_inline_image(content: &Value) -> bool {
    content["type"] == "input_image"
        && content["image_url"]
            .as_str()
            .is_some_and(|url| url.starts_with("data:"))
}

/// Preserve the latest image-bearing result and its sibling outputs/calls. A
/// batch may contain several parallel calls; keeping just the last result would
/// silently remove other images that the model has not inspected yet.
fn latest_image_batch_start(items: &[Value]) -> Option<usize> {
    let last_image = items.iter().rposition(|item| {
        is_tool_output(item)
            && item["output"]
                .as_array()
                // Reference images also establish a new image-bearing batch;
                // only the omission candidates need to be inline data URLs.
                .is_some_and(|contents| {
                    contents
                        .iter()
                        .any(|content| content["type"] == "input_image")
                })
    })?;
    let mut start = last_image;
    while start > 0 && is_tool_output(&items[start - 1]) {
        start -= 1;
    }
    // Include matching calls, even when parallel results arrived out of order.
    // Their original arguments retain file paths and page references for reloads.
    let call_ids: std::collections::HashSet<&str> = items[start..=last_image]
        .iter()
        .filter_map(|item| item["call_id"].as_str())
        .collect();
    if let Some(call_start) = items[..start].iter().position(|item| {
        is_tool_call(item)
            && item["call_id"]
                .as_str()
                .is_some_and(|id| call_ids.contains(id))
    }) {
        start = call_start;
    }
    while start > 0 && is_tool_call(&items[start - 1]) {
        start -= 1;
    }
    Some(start)
}

fn omit_older_tool_images(
    body: &mut Value,
    mut request_bytes: usize,
    target_bytes: usize,
) -> Result<usize, ApiError> {
    let Some(items) = body["input"].as_array_mut() else {
        return Ok(0);
    };
    let Some(protected_start) = latest_image_batch_start(items) else {
        return Ok(0);
    };
    let replacement = serde_json::json!({
        "type": "input_text",
        "text": "[Earlier tool image omitted from this request to stay within the image history byte budget. The original remains in local conversation history. Use the original tool call arguments or accompanying file/page references to reload it if needed. This image is no longer visible in the current model input.]"
    });
    let replacement_bytes = serde_json::to_vec(&replacement)
        .map_err(|_| invalid("Cannot encode image history notice"))?
        .len();
    let mut omitted = 0;
    // Oldest first, retaining call IDs, tool arguments, text, and content order.
    for item in &mut items[..protected_start] {
        if !is_tool_output(item) {
            continue;
        }
        for content in item["output"].as_array_mut().into_iter().flatten() {
            if request_bytes <= target_bytes {
                return Ok(omitted);
            }
            if !is_inline_image(content) {
                continue;
            }
            let image_bytes = serde_json::to_vec(content)
                .map_err(|_| invalid("Cannot encode image"))?
                .len();
            if image_bytes > replacement_bytes {
                request_bytes -= image_bytes - replacement_bytes;
                *content = replacement.clone();
                omitted += 1;
            }
        }
    }
    Ok(omitted)
}

fn encode(body: &Value) -> Result<EncodedJsonBody, ApiError> {
    EncodedJsonBody::encode(&RoutingFirst(body))
        .map_err(|_| invalid("Cannot encode Responses request"))
}

/// Keep gateway routing fields ahead of large inputs after preparing a JSON copy.
struct RoutingFirst<'a>(&'a Value);

impl serde::Serialize for RoutingFirst<'_> {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;

        let Some(object) = self.0.as_object() else {
            return self.0.serialize(serializer);
        };
        const PREFIX: &[&str] = &[
            "type",
            "model",
            "stream",
            "service_tier",
            "previous_response_id",
            "input",
        ];
        let mut map = serializer.serialize_map(Some(object.len()))?;
        for key in PREFIX {
            if let Some(value) = object.get(*key) {
                map.serialize_entry(key, value)?;
            }
        }
        for (key, value) in object {
            if !PREFIX.contains(&key.as_str()) {
                map.serialize_entry(key, value)?;
            }
        }
        map.end()
    }
}

fn invalid(message: impl Into<String>) -> ApiError {
    ApiError::InvalidRequest {
        message: message.into(),
    }
}

#[cfg(test)]
#[path = "request_budget_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "request_budget_history_tests.rs"]
mod history_tests;
