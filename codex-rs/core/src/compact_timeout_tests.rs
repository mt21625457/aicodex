//! 慢服务端导致压缩超时后，未完成摘要不能替换用户/助手/工具历史。
use super::*;
use crate::session::tests::make_session_and_context_with_auth_and_config_and_rx;
use codex_login::CodexAuth;
use codex_model_provider_info::ModelProviderInfo;
use core_test_support::responses;
use serde_json::json;
use std::time::Duration;
use wiremock::{
    Mock, ResponseTemplate,
    matchers::{method, path},
};

#[tokio::test]
async fn stalled_compaction_times_out_and_preserves_original_history() -> anyhow::Result<()> {
    let server = responses::start_mock_server().await;
    Mock::given(method("POST"))
        .and(path("/v1/responses"))
        .respond_with(ResponseTemplate::new(200).set_delay(Duration::from_secs(30)))
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
    let result = drain_to_completed(
        &session,
        &turn,
        &mut client,
        &metadata,
        &prompt,
        CompactionPhase::MidTurn,
        &mut crate::compaction_watchdog::CompactionWatchdog::almost_expired(),
    )
    .await;
    assert!(
        matches!(result, Err(ref error) if matches!(error.details(), CodexErrorDetails::CompactionTimeout))
    );
    let after = session.clone_history().await;
    assert_eq!(
        serde_json::to_value(before.raw_items().collect::<Vec<_>>())?,
        serde_json::to_value(after.raw_items().collect::<Vec<_>>())?
    );
    Ok(())
}
