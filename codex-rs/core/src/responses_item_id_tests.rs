use super::sanitize_overlong_item_ids;
use codex_protocol::ResponseItemId;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseItem;

fn message(length: usize) -> ResponseItem {
    ResponseItem::Message {
        id: Some(ResponseItemId::from_server(format!(
            "msg_{}",
            "x".repeat(length - 4)
        ))),
        role: "user".into(),
        content: vec![ContentItem::InputText {
            text: "keep me".into(),
        }],
        phase: None,
        internal_chat_message_metadata_passthrough: None,
    }
}

#[test]
fn boundary_lengths_and_repeated_preparation_preserve_history() {
    for length in [63, 64, 65, 83] {
        let history = vec![message(length)];
        let mut wire = history.clone();
        sanitize_overlong_item_ids(&mut wire).unwrap();
        let mut expected = history.clone();
        if length > 64 {
            expected[0].set_id(None);
        }
        assert_eq!(wire, expected);
        sanitize_overlong_item_ids(&mut wire).unwrap();
        assert_eq!(wire, expected);
        assert_eq!(history[0].id().unwrap().len(), length);
    }
}

#[test]
fn function_call_and_output_keep_correlation_and_payload() {
    let mut items = vec![
        ResponseItem::FunctionCall {
            id: Some(ResponseItemId::from_server(format!(
                "fc_{}",
                "x".repeat(80)
            ))),
            name: "exec_command".into(),
            namespace: Some("functions".into()),
            arguments: "{\"cmd\":\"pwd\"}".into(),
            encrypted_function_args: None,
            call_id: "call_keep_this".into(),
            internal_chat_message_metadata_passthrough: None,
        },
        ResponseItem::FunctionCallOutput {
            id: Some(ResponseItemId::from_server(format!(
                "fco_{}",
                "x".repeat(79)
            ))),
            call_id: Some("call_keep_this".into()),
            name: None,
            namespace: None,
            output: FunctionCallOutputPayload::from_text("result".into()),
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    let mut expected = items.clone();
    for item in &mut expected {
        item.set_id(None);
    }
    sanitize_overlong_item_ids(&mut items).unwrap();
    assert_eq!(items, expected);
}

#[test]
fn opaque_reasoning_fails_without_mutating_any_item_or_leaking_id() {
    let mut items = vec![
        message(83),
        ResponseItem::Reasoning {
            id: Some(ResponseItemId::from_server(format!(
                "rs_{}",
                "secret".repeat(14)
            ))),
            summary: vec![],
            content: None,
            encrypted_content: Some("ciphertext".into()),
            internal_chat_message_metadata_passthrough: None,
        },
    ];
    let original = items.clone();
    let error = sanitize_overlong_item_ids(&mut items).unwrap_err();
    assert!(error.contains("input[1].id (rs): length 87 exceeds 64"));
    assert!(!error.contains("secret"));
    assert!(!error.contains("ciphertext"));
    assert_eq!(items, original);
}

#[test]
fn encrypted_function_arguments_are_not_reidentified() {
    let mut items = vec![ResponseItem::FunctionCall {
        id: Some(ResponseItemId::from_server(format!(
            "fc_{}",
            "x".repeat(80)
        ))),
        name: "tool".into(),
        namespace: None,
        arguments: "{}".into(),
        encrypted_function_args: Some(vec!["ciphertext".into()]),
        call_id: "call_original".into(),
        internal_chat_message_metadata_passthrough: None,
    }];
    let original = items.clone();
    assert!(sanitize_overlong_item_ids(&mut items).is_err());
    assert_eq!(items, original);
}

#[test]
fn missing_ids_and_legal_unicode_ids_are_preserved() {
    let mut items = vec![message(64), message(64)];
    items[0].set_id(None);
    items[1].set_id(Some(ResponseItemId::from_server(format!(
        "msg_{}",
        "字".repeat(60)
    ))));
    let original = items.clone();
    sanitize_overlong_item_ids(&mut items).unwrap();
    assert_eq!(items, original);
}

#[test]
fn empty_message_and_unpaired_output_fail_closed() {
    let mut empty = message(83);
    if let ResponseItem::Message { content, .. } = &mut empty {
        content.clear();
    }
    assert!(sanitize_overlong_item_ids(&mut [empty]).is_err());
    let output = ResponseItem::FunctionCallOutput {
        id: Some(ResponseItemId::from_server(format!(
            "fco_{}",
            "x".repeat(79)
        ))),
        call_id: None,
        name: None,
        namespace: None,
        output: FunctionCallOutputPayload::from_text("result".into()),
        internal_chat_message_metadata_passthrough: None,
    };
    assert!(sanitize_overlong_item_ids(&mut [output]).is_err());
}
