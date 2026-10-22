use super::*;
use codex_protocol::models::ContentItem;

fn reasoning(text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::Reasoning {
        id: None,
        summary: vec![],
        content: Some(vec![ReasoningItemContent::Text { text: text.into() }]),
        encrypted_content: Some("foreign-provider-state".into()),
        internal_chat_message_metadata_passthrough: None,
    })
}

fn user(text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::Message {
        id: None,
        role: "user".into(),
        status: None,
        encrypted_content: None,
        content: vec![ContentItem::InputText { text: text.into() }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    })
}

fn text(item: &ResponseItemEnvelope) -> &str {
    let ResponseItem::Reasoning {
        content: Some(parts),
        ..
    } = &item.item
    else {
        panic!("expected reasoning");
    };
    match &parts[0] {
        ReasoningItemContent::Text { text } | ReasoningItemContent::ReasoningText { text } => text,
    }
}

#[test]
fn flash_responses_preserve_complete_large_items_and_source_history() {
    // 超过旧 8K 上限及新压缩目标；普通工具轮次仍完整回放，不以预算切断原文。
    let original = reasoning(&"完整推理".repeat(70_000));
    for model in [
        "deepseek-flash",
        "gateway/deepseek-flash",
        "deepseek/deepseek-flash:free",
        "aicodex_gateway_responses:deepseek-flash",
    ] {
        let mut input = vec![original.item.clone()];
        strip_reasoning_content_for_responses_input(
            &mut input,
            responses_reasoning_replay(model, false),
        );
        let wire = serde_json::to_value(&input[0]).unwrap();
        assert_eq!(wire["content"][0]["text"], text(&original));
        assert_eq!(wire["content"][0]["type"], "reasoning_text");
        assert!(wire.get("encrypted_content").is_none());
    }
    assert!(
        serde_json::to_value(&original.item)
            .unwrap()
            .get("encrypted_content")
            .is_some()
    );
}

#[test]
fn raw_model_policies_preserve_complete_content_and_other_models_keep_encrypted_handoff() {
    for model in ["deepseek-v4-pro", "deepseek-flash-next", "MiniMax-M3"] {
        assert_eq!(
            responses_reasoning_replay(model, false),
            ResponsesReasoningReplay::RawReasoningText
        );
        let mut input = vec![reasoning(&"a".repeat(80_000)).item];
        strip_reasoning_content_for_responses_input(
            &mut input,
            responses_reasoning_replay(model, false),
        );
        let ResponseItem::Reasoning { content, .. } = &input[0] else {
            panic!("reasoning");
        };
        assert_eq!(
            raw_reasoning_content_tokens(content),
            approx_token_count(&"a".repeat(80_000))
        );
    }
    for model in ["gpt-6.1-sol", "grok-4.7", "claude-sonnet"] {
        assert_eq!(
            responses_reasoning_replay(model, true),
            ResponsesReasoningReplay::EncryptedHandoff {
                is_openai_provider: true
            }
        );
    }
}

#[test]
fn compaction_keeps_newest_whole_items_and_ignores_summary_as_user_boundary() {
    let older = "a".repeat(160_000);
    let newest = "b".repeat(160_000);
    let items = vec![
        user("old task"),
        reasoning("old task reasoning"),
        user("current task"),
        reasoning(&older),
        reasoning(&newest),
        user(&format!("{SUMMARY_PREFIX}\nsummary")),
    ];
    let before: Vec<_> = items.iter().map(|envelope| envelope.item.clone()).collect();
    let retained = last_turn_reasoning_for_raw_replay(&items, "deepseek-flash", false);
    assert_eq!(retained.len(), 1);
    assert_eq!(text(&retained[0]), newest);
    assert_eq!(
        items
            .iter()
            .map(|envelope| envelope.item.clone())
            .collect::<Vec<_>>(),
        before
    );
}

#[test]
fn compaction_preserves_oversized_newest_item_instead_of_partial_reasoning() {
    let newest = "n".repeat(300_000);
    let items = vec![user("task"), reasoning("earlier"), reasoning(&newest)];
    let retained = last_turn_reasoning_for_raw_replay(&items, "deepseek-flash", false);
    assert_eq!(retained.len(), 1);
    assert_eq!(text(&retained[0]), newest);
    assert!(
        last_turn_complete_reasoning(&[reasoning("no user boundary")], "deepseek-flash").is_empty()
    );
}

#[test]
fn compaction_retains_order_when_multiple_complete_items_fit() {
    let items = vec![user("task"), reasoning("first"), reasoning("second")];
    let retained = last_turn_complete_reasoning(&items, "deepseek-flash");
    assert_eq!(
        retained.iter().map(text).collect::<Vec<_>>(),
        vec!["first", "second"]
    );
}

#[test]
fn all_raw_models_preserve_oversized_complete_items_after_compaction() {
    let original = reasoning(&"完整推理".repeat(70_000));
    for slug in [
        "deepseek-flash",
        "deepseek-v4-flash",
        "deepseek-v4-pro",
        "deepseek/deepseek-flash:free",
        "gateway/deepseek-v4-pro:free",
        "MiniMax-M3",
        "minimax/MiniMax-M3:free",
        "gateway:MiniMax-M3",
    ] {
        let items = vec![user("task"), original.clone()];
        let retained = last_turn_reasoning_for_raw_replay(&items, slug, false);
        assert_eq!(retained.len(), 1, "missing reasoning for {slug}");
        assert_eq!(
            text(&retained[0]),
            text(&original),
            "truncated reasoning for {slug}"
        );
    }
}
