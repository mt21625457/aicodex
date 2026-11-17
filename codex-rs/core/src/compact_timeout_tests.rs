//! 慢服务端导致压缩超时后，未完成摘要不能替换用户/助手/工具历史。
use super::*;
use crate::session::tests::make_session_and_context_with_auth_and_config_and_rx;
use codex_login::CodexAuth;
use codex_model_provider_info::ModelProviderInfo;
use core_test_support::responses;
use serde_json::json;
use std::time::Duration;
use wiremock::Mock;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

#[test_case::test_case("timeout")]
#[test_case::test_case("cancel")]
#[test_case::test_case("incomplete")]
#[tokio::test]
async fn failed_compaction_preserves_original_history(mode: &str) -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    let response = if mode == "incomplete" {
        ResponseTemplate::new(200)
            .insert_header("content-type", "text/event-stream")
            .set_body_string(responses::sse(vec![
                responses::ev_assistant_message("partial", "must not replace original history"),
                json!({"type":"response.incomplete", "response":{
                    "id":"synthetic-incomplete", "incomplete_details":{"reason":"max_output_tokens"}
                }}),
            ]))
    } else {
        ResponseTemplate::new(200).set_delay(Duration::from_secs(30))
    };
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(response)
        .mount(&server)
        .await;
    let provider = ModelProviderInfo::create_openai_provider(Some(format!("{}/v1", server.uri())));
    let (session, turn, _events) = make_session_and_context_with_auth_and_config_and_rx(
        CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        Vec::new(),
        move |config| {
            config.model = Some("gpt-5.2".to_string());
            config.model_provider = provider;
            config.model_provider.supports_websockets = false;
        },
    )
    .await;
    let items = vec![
        json!({"type":"message","role":"user","content":[{"type":"input_text","text":"keep my request"}]}),
        json!({"type":"message","role":"assistant","content":[{"type":"output_text","text":"observed answer"}]}),
        json!({"type":"function_call","name":"test_tool","arguments":"{}","call_id":"test-call"}),
        json!({"type":"function_call_output","call_id":"test-call","output":"unique result"}),
    ].into_iter().map(serde_json::from_value).collect::<serde_json::Result<Vec<ResponseItem>>>()?;
    session
        .record_conversation_items(&turn, turn.model_info(), &items)
        .await;
    let before = session.clone_history().await;
    let mut client = session.services.model_client.new_session();
    let metadata = session
        .compaction_responses_metadata(
            &turn,
            CompactionTurnMetadata::new(
                CompactionTrigger::Auto,
                CompactionReason::ContextLimit,
                CompactionImplementation::Responses,
                CompactionPhase::MidTurn,
            ),
        )
        .await;
    let prompt = Prompt {
        input: items,
        ..Default::default()
    };
    let mut watchdog = if mode == "timeout" {
        crate::compaction_watchdog::CompactionWatchdog::almost_expired()
    } else {
        // incomplete验证服务端终态，不让200ms测试截止与本机调度竞争。
        crate::compaction_watchdog::CompactionWatchdog::new()
    };
    let request = drain_to_completed(
        &session,
        &turn,
        &mut client,
        &metadata,
        &prompt,
        CompactionPhase::MidTurn,
        &mut watchdog,
    );
    // 取消通过丢弃网络future传播；晚到的响应不能再触及历史提交。
    if mode == "cancel" {
        assert!(
            tokio::time::timeout(Duration::from_millis(20), request)
                .await
                .is_err()
        );
    } else {
        let error = request.await.err().expect("failure must remain observable");
        if mode == "incomplete" {
            assert!(matches!(
                error.details(),
                CodexErrorDetails::ResponseIncomplete(_)
            ));
        } else {
            assert!(matches!(
                error.details(),
                CodexErrorDetails::CompactionTimeout
            ));
        }
    }
    let after = session.clone_history().await;
    assert_eq!(
        serde_json::to_value(before.raw_items().collect::<Vec<_>>())?,
        serde_json::to_value(after.raw_items().collect::<Vec<_>>())?
    );
    Ok(())
}
