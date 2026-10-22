//! 在真实 Responses 请求构造边界验证 Flash，防止 helper 正确但出站路径仍截断。
use super::*;
use pretty_assertions::assert_eq;

#[test]
fn responses_request_preserves_flash_reasoning_and_explicit_effort_without_changing_pro() {
    let client = test_model_client(SessionSource::Cli);
    let full_reasoning = "r".repeat(80_000);
    let mut item = reasoning_item("raw-reasoning");
    if let ResponseItem::Reasoning { content, .. } = &mut item {
        *content = Some(vec![ReasoningItemContent::ReasoningText {
            text: full_reasoning.clone(),
        }]);
    }
    let prompt = Prompt {
        input: vec![item],
        ..Prompt::default()
    };
    for (slug, complete) in [
        ("deepseek-flash", true),
        ("deepseek/deepseek-flash:free", true),
        ("deepseek-v4-pro", false),
        ("MiniMax-M3", false),
    ] {
        let mut model = test_model_info();
        model.slug = slug.into();
        model.context_window = Some(1_048_576);
        model.max_output_tokens = Some(131_072);
        model.auto_compact_token_limit = complete.then_some(750_000);
        model.default_reasoning_level = Some(codex_protocol::openai_models::ReasoningEffort::High);
        let request = client
            .build_responses_request(
                &prompt,
                &model,
                Some(codex_protocol::openai_models::ReasoningEffort::Max),
                codex_protocol::config_types::ReasoningSummary::None,
                None,
                &test_responses_metadata_for_client(
                    &client,
                    None,
                    format!("{}:0", client.state.thread_id),
                    None,
                    TestCodexResponsesRequestKind::Turn,
                ),
                true,
            )
            .unwrap();
        let wire = serde_json::to_value(&request).unwrap();
        let reasoning = wire["input"]
            .as_array()
            .unwrap()
            .iter()
            .find(|item| item["type"] == "reasoning")
            .unwrap();
        let replayed = reasoning["content"][0]["text"].as_str().unwrap();
        if complete {
            assert_eq!(replayed, full_reasoning);
        } else {
            assert!(replayed.len() < full_reasoning.len());
        }
        assert_eq!(
            wire["reasoning"]["effort"], "max",
            "显式 max 必须优先于新默认"
        );
        assert_eq!(wire["max_output_tokens"], 131_072);
        assert_eq!(
            model.auto_compact_token_limit(),
            Some(if complete { 750_000 } else { 917_504 })
        );
    }
    let original = serde_json::to_value(&prompt.input[0]).unwrap();
    assert_eq!(original["content"][0]["text"], full_reasoning);
}
