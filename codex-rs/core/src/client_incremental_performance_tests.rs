use super::*;

#[test]
fn borrowed_incremental_normalization_matches_whole_history_for_every_policy() {
    // 混合大正文与超长 reasoning，验证增量规范化与完整历史的回放策略一致，且不复制普通历史。
    let items: Vec<ResponseItem> = vec![
        serde_json::from_value(serde_json::json!({"type":"message","role":"user","content":[{"type":"input_text","text":"正文".repeat(20_000)}]})).unwrap(),
        serde_json::from_value(serde_json::json!({"type":"reasoning","id":"rs_test","summary":[],"content":[{"type":"reasoning_text","text":"reasoning ".repeat(40_000)}],"encrypted_content":"opaque"})).unwrap(),
        serde_json::from_value(serde_json::json!({"type":"reasoning","summary":[],"content":[{"type":"reasoning_text","text":"tail reasoning"}]})).unwrap(),
    ];
    for policy in [
        ResponsesReasoningReplay::EncryptedHandoff {
            is_openai_provider: true,
        },
        ResponsesReasoningReplay::EncryptedHandoff {
            is_openai_provider: false,
        },
        ResponsesReasoningReplay::RawReasoningText,
    ] {
        let mut expected = items.clone();
        strip_reasoning_content_for_responses_input(&mut expected, policy);
        for (original, expected) in items.iter().zip(&expected) {
            let actual = normalized_incremental_item(original, policy);
            assert_eq!(actual.as_ref(), expected);
            if !matches!(original, ResponseItem::Reasoning { .. }) {
                assert!(matches!(actual, std::borrow::Cow::Borrowed(_)));
                assert!(std::ptr::eq(actual.as_ref(), original));
            }
        }
    }
}
