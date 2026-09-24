use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

#[test]
fn structured_mcp_results_preserve_media_and_status() {
    for media in [
        json!({"type":"image","data":"BASE64","mimeType":"image/png"}),
        json!({"type":"audio","data":"BASE64","mimeType":"audio/wav"}),
    ] {
        for is_error in [false, true] {
            let structured = json!({"window_id":42,"snapshot_id":"snapshot-1"});
            let content = vec![json!({"type":"text","text":"caption"}), media.clone()];
            let mut expected = convert_mcp_content_to_items(&content);
            expected.push(FunctionCallOutputContentItem::InputText {
                text: structured.to_string(),
            });
            let result = CallToolResult {
                content,
                structured_content: Some(structured),
                is_error: Some(is_error),
                meta: None,
            };
            assert_eq!(
                result.as_function_call_output_payload(),
                FunctionCallOutputPayload {
                    body: FunctionCallOutputBody::ContentItems(expected),
                    success: Some(!is_error),
                }
            );
            let wire = serde_json::to_value(result.into_function_call_output_payload()).unwrap();
            assert_eq!(
                wire[1]["type"],
                if media["type"] == "image" {
                    "input_image"
                } else {
                    "input_audio"
                }
            );
            assert_eq!(wire.as_array().unwrap().len(), 3);
        }
    }
}
