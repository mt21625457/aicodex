use super::*;
use pretty_assertions::assert_eq;
use serde_json::json;

fn completed() -> ResponseEvent {
    ResponseEvent::Completed {
        response_id: "sample".to_string(),
        token_usage: None,
        usage_metadata: None,
        end_turn: Some(true),
        provider_stop_reason: None,
    }
}

fn cited_output(text: &str) -> ResponseEvent {
    ResponseEvent::OutputItemDone(
        serde_json::from_value(json!({
            "type": "message", "role": "assistant", "content": [{
                "type": "output_text_with_citations", "text": text,
                "citations": [{"provider": "archive", "type": "url", "citedText": "evidence"}]
            }]
        }))
        .unwrap(),
    )
}

#[tokio::test]
async fn sample_collects_cited_text_and_enforces_the_same_output_limit() {
    let stream = Box::pin(futures::stream::iter(vec![
        Ok(cited_output("Cited answer.")),
        Ok(completed()),
    ]));
    assert_eq!(
        collect_response(stream).await.unwrap().text.as_deref(),
        Some("Cited answer.")
    );

    let stream = Box::pin(futures::stream::iter(vec![
        Ok(cited_output(&"x".repeat(MAX_OUTPUT_BYTES + 1))),
        Ok(completed()),
    ]));
    assert!(
        collect_response(stream)
            .await
            .unwrap_err()
            .message
            .contains("sample output exceeds limit")
    );
}

#[tokio::test]
async fn sample_cited_text_does_not_hide_a_tool_attempt() {
    let tool = serde_json::from_value(json!({
        "type": "function_call", "name": "shell", "call_id": "call", "arguments": "{}"
    }))
    .unwrap();
    let stream = Box::pin(futures::stream::iter(vec![
        Ok(cited_output("Cited answer.")),
        Ok(ResponseEvent::OutputItemAdded(tool)),
        Ok(completed()),
    ]));
    assert_eq!(collect_response(stream).await.unwrap().text, None);
}
