use super::*;
use codex_history::ResponseItemEnvelope;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::ReasoningItemContent;
use codex_protocol::models::ReasoningItemReasoningSummary;
use codex_protocol::models::ResponseItem;
use pretty_assertions::assert_eq;

fn user_message(text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::Message {
        id: None,
        role: "user".to_string(),
        content: vec![ContentItem::InputText {
            text: text.to_string(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    })
}

fn reasoning_item(id: &str, text: &str) -> ResponseItemEnvelope {
    ResponseItemEnvelope::new(ResponseItem::Reasoning {
        id: Some(ResponseItemId::from_server(id.to_string())),
        summary: Vec::new(),
        content: Some(vec![ReasoningItemContent::ReasoningText {
            text: text.to_string(),
        }]),
        encrypted_content: Some(format!("encrypted-{id}")),
        internal_chat_message_metadata_passthrough: None,
    })
}

#[test]
fn non_deepseek_models_keep_encrypted_handoff_split_by_provider() {
    assert_eq!(
        responses_reasoning_replay("gpt-5.4", /*is_openai_provider*/ true),
        ResponsesReasoningReplay::EncryptedHandoff {
            is_openai_provider: true
        }
    );
    assert_eq!(
        responses_reasoning_replay("grok-4.5", /*is_openai_provider*/ false),
        ResponsesReasoningReplay::EncryptedHandoff {
            is_openai_provider: false
        }
    );
}

#[test]
fn minimax_and_deepseek_use_raw_reasoning_replay() {
    for slug in [
        "MiniMax-M3",
        "minimax:MiniMax-M3",
        "aicodex_gateway_responses:MiniMax-M3",
        "minimax/MiniMax-M3:free",
        "deepseek-v4-pro",
    ] {
        assert_eq!(
            responses_reasoning_replay(slug, /*is_openai_provider*/ false),
            ResponsesReasoningReplay::RawReasoningText,
            "unexpected replay policy for {slug}"
        );
    }
}

#[test]
fn raw_replay_normalizes_legacy_text_and_clears_provider_state() {
    let mut input = vec![ResponseItem::Reasoning {
        id: Some(ResponseItemId::from_server("reasoning-id".to_string())),
        summary: vec![ReasoningItemReasoningSummary::SummaryText {
            text: "summary".to_string(),
        }],
        content: Some(vec![ReasoningItemContent::Text {
            text: "keep this chain of thought".to_string(),
        }]),
        encrypted_content: Some("provider-specific-state".to_string()),
        internal_chat_message_metadata_passthrough: None,
    }];

    strip_reasoning_content_for_responses_input(
        &mut input,
        ResponsesReasoningReplay::RawReasoningText,
    );

    let serialized = serde_json::to_value(&input[0]).expect("serialize reasoning item");
    assert_eq!(serialized["type"], "reasoning");
    assert_eq!(
        serialized["content"][0]["text"].as_str(),
        Some("keep this chain of thought")
    );
    assert_eq!(serialized["content"][0]["type"], "reasoning_text");
    assert!(serialized.get("id").is_none());
    assert_eq!(serialized["summary"], serde_json::json!([]));
    assert!(serialized.get("encrypted_content").is_none());
}

#[test]
fn raw_replay_caps_each_reasoning_and_normalizes_missing_content() {
    let oversized = "n".repeat((RAW_REASONING_REPLAY_ITEM_MAX_TOKENS + 100) * 4);
    let mut input = vec![
        reasoning_item("old", "old thought").item,
        reasoning_item("middle", "middle thought").item,
        reasoning_item("new", &oversized).item,
    ];
    if let ResponseItem::Reasoning { content, .. } = &mut input[0] {
        *content = None;
    }

    strip_reasoning_content_for_responses_input(
        &mut input,
        ResponsesReasoningReplay::RawReasoningText,
    );

    let ResponseItem::Reasoning {
        content: old_content,
        encrypted_content: old_encrypted_content,
        ..
    } = &input[0]
    else {
        panic!("expected old reasoning item");
    };
    assert_eq!(old_content, &Some(Vec::new()));
    assert_eq!(old_encrypted_content, &None);

    let middle = serde_json::to_value(&input[1]).expect("serialize middle reasoning item");
    assert!(middle.get("content").is_some());

    let ResponseItem::Reasoning {
        content: Some(new_content),
        encrypted_content: new_encrypted_content,
        ..
    } = &input[2]
    else {
        panic!("expected new reasoning item with content");
    };
    let retained_tokens = raw_reasoning_content_tokens(&Some(new_content.clone()));
    assert!(retained_tokens > 0);
    assert!(retained_tokens <= RAW_REASONING_REPLAY_ITEM_MAX_TOKENS);
    assert!(retained_tokens < approx_token_count(&oversized));
    assert_eq!(new_encrypted_content, &None);
}

#[test]
fn last_turn_reasoning_keeps_only_items_after_the_last_real_user() {
    let items = vec![
        user_message("first"),
        reasoning_item("old", "old thought"),
        user_message("second"),
        reasoning_item("new", "new thought"),
    ];

    for slug in ["deepseek-v4-pro", "MiniMax-M3"] {
        let retained =
            last_turn_reasoning_for_raw_replay(&items, slug, /*is_openai_provider*/ false);

        assert_eq!(retained.len(), 1, "unexpected retained count for {slug}");
        match &retained[0].item {
            ResponseItem::Reasoning { content, .. } => {
                assert_eq!(
                    content,
                    &Some(vec![ReasoningItemContent::ReasoningText {
                        text: "new thought".to_string(),
                    }])
                );
            }
            other => panic!("expected reasoning item for {slug}, got {other:?}"),
        }
    }
}

#[test]
fn last_turn_reasoning_prefers_newest_items_when_over_budget() {
    let items = vec![
        user_message("hello"),
        reasoning_item("old", &"a".repeat(40_000)),
        reasoning_item("new", "short thought"),
    ];

    let retained = last_turn_reasoning_for_raw_replay(
        &items,
        "deepseek-v4-pro",
        /*is_openai_provider*/ false,
    );

    assert_eq!(retained.len(), 1);
    match &retained[0].item {
        ResponseItem::Reasoning { id, content, .. } => {
            assert_eq!(id, &None);
            assert_eq!(
                content,
                &Some(vec![ReasoningItemContent::ReasoningText {
                    text: "short thought".to_string(),
                }])
            );
        }
        other => panic!("expected newest reasoning item, got {other:?}"),
    }
}

#[test]
fn last_turn_reasoning_truncates_an_oversized_newest_item_to_the_hard_cap() {
    let oversized = "n".repeat((RAW_REASONING_REPLAY_ITEM_MAX_TOKENS + 100) * 4);
    let items = vec![user_message("hello"), reasoning_item("new", &oversized)];

    let retained = last_turn_reasoning_for_raw_replay(
        &items,
        "deepseek-v4-pro",
        /*is_openai_provider*/ false,
    );

    let [
        ResponseItemEnvelope {
            item:
                ResponseItem::Reasoning {
                    content: Some(content),
                    encrypted_content,
                    ..
                },
            ..
        },
    ] = retained.as_slice()
    else {
        panic!("expected one retained reasoning item");
    };
    let retained_tokens = raw_reasoning_content_tokens(&Some(content.clone()));
    assert!(retained_tokens > 0);
    assert!(retained_tokens <= RAW_REASONING_REPLAY_ITEM_MAX_TOKENS);
    assert!(retained_tokens < approx_token_count(&oversized));
    assert_eq!(encrypted_content, &None);
}
