//! 在真实 Responses 请求构造边界验证所有原始推理模型，防止出站路径截断。
use super::*;
use pretty_assertions::assert_eq;

#[test]
fn responses_request_preserves_all_raw_reasoning_and_explicit_effort() {
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
    for (slug, explicit_compact_limit) in [
        ("deepseek-flash", true),
        ("deepseek/deepseek-flash:free", true),
        ("deepseek-v4-pro", false),
        ("MiniMax-M3", false),
        ("minimax/MiniMax-M3:free", false),
        ("deepseek-v4-flash", false),
        ("gateway:deepseek-v4-pro", false),
    ] {
        let mut model = test_model_info();
        model.slug = slug.into();
        model.context_window = Some(1_048_576);
        model.max_output_tokens = Some(131_072);
        model.auto_compact_token_limit = explicit_compact_limit.then_some(750_000);
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
        assert_eq!(replayed, full_reasoning, "truncated reasoning for {slug}");
        assert_eq!(
            wire["reasoning"]["effort"], "max",
            "显式 max 必须优先于新默认"
        );
        assert_eq!(wire["max_output_tokens"], 131_072);
        assert_eq!(
            model.auto_compact_token_limit(),
            Some(if explicit_compact_limit {
                750_000
            } else {
                917_504
            })
        );
    }
    let original = serde_json::to_value(&prompt.input[0]).unwrap();
    assert_eq!(original["content"][0]["text"], full_reasoning);
}
