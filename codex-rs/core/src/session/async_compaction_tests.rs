//! Cancellation and provider changes preserve ownership of speculative compaction.

use super::*;
use codex_analytics::CompactionReason;
use codex_analytics::CompactionTrigger;
use codex_login::CodexAuth;
use codex_protocol::models::ResponseItem;
use codex_protocol::protocol::TokenUsage;
use core_test_support::responses;
use pretty_assertions::assert_eq;

#[test_case::test_case(CompactionTrigger::Auto; "optional ready adoption")]
#[test_case::test_case(CompactionTrigger::Manual; "manual ready reuse")]
#[tokio::test]
async fn cancelled_turn_skips_launch_but_preserves_existing_work(trigger: CompactionTrigger) {
    let phase = CompactionPhase::MidTurn;
    let server = responses::start_mock_server().await;
    let mut provider =
        codex_model_provider_info::built_in_model_providers(/*openai_base_url*/ None)["openai"]
            .clone();
    provider.base_url = Some(format!("{}/v1", server.uri()));
    provider.supports_websockets = false;
    let server_uri = server.uri();
    let (sess, turn, _events) =
        crate::session::tests::make_session_and_context_with_auth_and_config_and_rx(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            Vec::new(),
            move |config| {
                config.model = Some("gpt-5.2".to_string());
                config.model_context_window = Some(100_000);
                config.model_auto_compact_token_limit = Some(90_000);
                config.model_provider = provider;
                config.chatgpt_base_url = server_uri;
                config.features.enable(Feature::AsyncCompaction).unwrap();
                config.features.disable(Feature::TokenBudget).unwrap();
            },
        )
        .await;
    let request = responses::mount_sse_once(
        &server,
        responses::sse(vec![
            serde_json::json!({"type":"response.output_item.done", "item": {
                "type":"compaction", "encrypted_content":"prepared-summary"
            }}),
            responses::ev_completed("prepared"),
        ]),
    )
    .await;
    let turn_cancellation = CancellationToken::new();
    let step = sess
        .capture_step_context(Arc::clone(&turn), &turn_cancellation)
        .await
        .expect("capture step");
    let original: ResponseItem = serde_json::from_value(serde_json::json!({
        "type": "message", "role": "user",
        "content": [{"type": "input_text", "text": "original turn"}]
    }))
    .expect("user message");
    sess.replace_history(vec![original], /*reference_context_item*/ None)
        .await;
    sess.update_token_usage_info(
        &turn,
        Some(&TokenUsage {
            input_tokens: 70_000,
            total_tokens: 70_000,
            ..Default::default()
        }),
    )
    .await
    .expect("record usage");
    let status = super::super::context_window::context_window_token_status(&sess, &turn).await;
    let world_state = Arc::new(WorldState::default());
    let cancelled = CancellationToken::new();
    cancelled.cancel();
    let auth = sess.services.model_client.auth_owner_generation();
    maybe_start(
        &sess,
        &step,
        &world_state,
        phase,
        &cancelled,
        &status,
        (Some("routing".to_string()), auth),
    )
    .await;
    assert!(
        sess.state
            .lock()
            .await
            .async_compaction
            .last_attempt
            .is_none()
    );
    assert_eq!(request.requests().len(), 0);

    // The same eligible work can launch on a live turn, then outlive its cancellation.
    maybe_start(
        &sess,
        &step,
        &world_state,
        phase,
        &turn_cancellation,
        &status,
        (Some("routing".to_string()), auth),
    )
    .await;
    turn_cancellation.cancel();
    maybe_start(
        &sess,
        &step,
        &world_state,
        phase,
        &turn_cancellation,
        &status,
        (Some("routing".to_string()), auth),
    )
    .await;
    // Wait for client publication before exercising either ready-result consumer.
    wait_for_background_compaction_for_test(&sess).await;
    let next_step = sess
        .capture_step_context(Arc::clone(&turn), &CancellationToken::new())
        .await
        .expect("capture next step");
    match trigger {
        CompactionTrigger::Auto => {
            let candidate = take_ready(&sess, &turn).await.expect("published candidate");
            let mut client_session = sess.services.model_client.new_session();
            assert!(
                crate::compact_remote_v2::run_inline_remote_auto_compact_task(
                    Arc::clone(&sess),
                    Arc::clone(&next_step),
                    next_step,
                    &mut client_session,
                    world_state,
                    CompactionReason::ContextLimit,
                    phase,
                    Some(candidate),
                    crate::compact_remote_v2::CompactionFallback::Skip,
                )
                .await
                .expect("optional adoption")
            );
        }
        CompactionTrigger::Manual => {
            crate::compact_remote_v2::run_remote_compact_task(
                Arc::clone(&sess),
                next_step,
                world_state,
            )
            .await
            .expect("manual compaction reuses the completed request");
        }
    }
    let history = sess.clone_history().await;
    let summaries = history
        .raw_items()
        .filter_map(|item| match item {
            ResponseItem::Compaction {
                encrypted_content, ..
            } => Some(encrypted_content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(summaries, ["prepared-summary"]);
    assert_eq!(request.requests().len(), 1);
    shutdown(&sess).await;
}

#[test_case::test_case(CompactionTrigger::Auto; "ready candidate")]
#[test_case::test_case(CompactionTrigger::Manual; "pending candidate")]
#[tokio::test]
async fn provider_switch_rejects_previous_candidate_and_routes_new_attempt(
    trigger: CompactionTrigger,
) {
    let server_a = responses::start_mock_server().await;
    let server_b = responses::start_mock_server().await;
    let mut provider =
        codex_model_provider_info::built_in_model_providers(/*openai_base_url*/ None)["openai"]
            .clone();
    provider.base_url = Some(format!("{}/v1", server_a.uri()));
    provider.supports_websockets = false;
    let server_uri = server_a.uri();
    let (sess, mut turn, _events) =
        crate::session::tests::make_session_and_context_with_auth_and_config_and_rx(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
            Vec::new(),
            move |config| {
                config.model = Some("gpt-5.2".to_string());
                config.model_context_window = Some(100_000);
                config.model_auto_compact_token_limit = Some(90_000);
                config.model_provider = provider;
                config.chatgpt_base_url = server_uri;
                config.features.enable(Feature::AsyncCompaction).unwrap();
                config.features.disable(Feature::TokenBudget).unwrap();
            },
        )
        .await;
    let initial_turn = Arc::get_mut(&mut turn).expect("unshared initial turn");
    let settings = Arc::make_mut(&mut initial_turn.initial_settings);
    Arc::make_mut(&mut settings.model_info).comp_hash = Some("shared-compaction-hash".to_string());
    initial_turn
        .next_step_settings
        .store(Arc::clone(&initial_turn.initial_settings));
    for (server, summary) in [(&server_a, "summary-from-a"), (&server_b, "summary-from-b")] {
        responses::mount_sse_once(
            server,
            responses::sse(vec![
                serde_json::json!({"type":"response.output_item.done", "item": {
                    "type":"compaction", "encrypted_content": summary
                }}),
                responses::ev_completed(summary),
            ]),
        )
        .await;
    }
    let cancellation = CancellationToken::new();
    let step_a = sess
        .capture_step_context(Arc::clone(&turn), &cancellation)
        .await
        .expect("capture provider A step");
    let original: ResponseItem = serde_json::from_value(serde_json::json!({
        "type": "message", "role": "user",
        "content": [{"type": "input_text", "text": "history shared across providers"}]
    }))
    .expect("user message");
    sess.replace_history(vec![original], /*reference_context_item*/ None)
        .await;
    sess.update_token_usage_info(
        &turn,
        Some(&TokenUsage {
            input_tokens: 70_000,
            total_tokens: 70_000,
            ..Default::default()
        }),
    )
    .await
    .expect("record usage");
    let status = super::super::context_window::context_window_token_status(&sess, &turn).await;
    let world_state = Arc::new(WorldState::default());
    let auth = provider_auth_owner_generation(&turn);
    let model_a = Arc::clone(&step_a.settings.model_info);
    maybe_start(
        &sess,
        &step_a,
        &world_state,
        CompactionPhase::MidTurn,
        &cancellation,
        &status,
        (Some("routing-a".to_string()), auth),
    )
    .await;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        wait_for_background_compaction_for_test(&sess),
    )
    .await
    .expect("provider A background request completes");
    assert_eq!(
        responses::received_responses_requests(&server_a)
            .await
            .len(),
        1
    );
    assert_eq!(
        responses::received_responses_requests(&server_b)
            .await
            .len(),
        0
    );
    let history_version = {
        let state = sess.state.lock().await;
        assert!(
            !state
                .async_compaction
                .result
                .as_ref()
                .expect("provider A registered a result")
                .is_empty(),
            "provider A must publish a candidate before testing its rejection"
        );
        state.history.history_version()
    };

    // The session's default client remains on A, while the next turn selects B.
    // Keeping model, hash, history and auth identical isolates provider identity.
    drop(step_a);
    let turn_b = Arc::get_mut(&mut turn).expect("completed background task releases the turn");
    let config = Arc::make_mut(&mut turn_b.config);
    config.model_provider_id = "provider-b".to_string();
    config.model_provider.base_url = Some(format!("{}/v1", server_b.uri()));
    turn_b.provider = codex_model_provider::create_model_provider(
        config.model_provider.clone(),
        turn_b.auth_manager.clone(),
    );
    {
        // Accepted provider changes also update the session snapshot checked by
        // prefix installation; the long-lived model client still belongs to A.
        let mut state = sess.state.lock().await;
        state.session_configuration.provider_id = turn_b.config.model_provider_id.clone();
        state.session_configuration.provider = Arc::clone(&turn_b.provider);
    }
    let step_b = sess
        .capture_step_context(Arc::clone(&turn), &cancellation)
        .await
        .expect("capture provider B step");
    assert_eq!(step_b.settings.model_info.slug, model_a.slug);
    assert_eq!(step_b.settings.model_info.comp_hash, model_a.comp_hash);
    assert_eq!(provider_auth_owner_generation(&turn), auth);
    assert_eq!(
        sess.state.lock().await.history.history_version(),
        history_version
    );
    assert_ne!(
        sess.services.model_client.provider_info(),
        turn.provider.info()
    );
    let previous_candidate = match trigger {
        CompactionTrigger::Auto => take_ready(&sess, &turn).await,
        CompactionTrigger::Manual => wait_for_pending(&sess, &step_b).await,
    };
    assert!(
        previous_candidate.is_none(),
        "provider B must reject provider A's candidate"
    );

    // The unchanged history must not suppress a new attempt for provider B.
    maybe_start(
        &sess,
        &step_b,
        &world_state,
        CompactionPhase::MidTurn,
        &cancellation,
        &status,
        (Some("routing-b".to_string()), auth),
    )
    .await;
    tokio::time::timeout(
        std::time::Duration::from_secs(30),
        wait_for_background_compaction_for_test(&sess),
    )
    .await
    .expect("provider B background request completes");
    let candidate = take_ready(&sess, &turn)
        .await
        .expect("provider B publishes a new candidate for the same history");
    let mut client_session = sess
        .services
        .model_client
        .new_session_for_provider(Arc::clone(&turn.provider));
    assert!(
        crate::compact_remote_v2::run_inline_remote_auto_compact_task(
            Arc::clone(&sess),
            Arc::clone(&step_b),
            step_b,
            &mut client_session,
            world_state,
            CompactionReason::ContextLimit,
            CompactionPhase::MidTurn,
            Some(candidate),
            crate::compact_remote_v2::CompactionFallback::Skip,
        )
        .await
        .expect("adopt provider B's candidate")
    );
    let history = sess.clone_history().await;
    let summaries = history
        .raw_items()
        .filter_map(|item| match item {
            ResponseItem::Compaction {
                encrypted_content, ..
            } => Some(encrypted_content.as_str()),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(summaries, ["summary-from-b"]);
    assert_eq!(
        responses::received_responses_requests(&server_a)
            .await
            .len(),
        1
    );
    assert_eq!(
        responses::received_responses_requests(&server_b)
            .await
            .len(),
        1
    );
    shutdown(&sess).await;
}
