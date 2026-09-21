use anyhow::Result;
use codex_features::Feature;
use core_test_support::responses::ev_assistant_message;
use core_test_support::responses::ev_completed_with_tokens;
use core_test_support::responses::ev_function_call;
use core_test_support::responses::mount_sse_sequence;
use core_test_support::responses::sse;
use core_test_support::responses::sse_failed;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::test_codex;
use pretty_assertions::assert_eq;
use serde_json::json;
use test_case::test_case;

#[test_case("gpt-5.2"; "gpt")]
#[test_case("deepseek-flash"; "deepseek")]
#[test_case("grok-4.5"; "grok")]
#[test_case("MiniMax-M3"; "minimax")]
#[test_case("custom-model"; "unknown model")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn automatic_compaction_preserves_continuation_guidance(model: &str) -> Result<()> {
    let server = start_mock_server().await;
    let mut bodies = Vec::new();
    for index in 0..2 {
        bodies.push(sse(vec![
            ev_function_call(
                &format!("plan-{index}"),
                "update_plan",
                &json!({"plan": [{"step": "Finish all requested work", "status": "in_progress"}]})
                    .to_string(),
            ),
            ev_completed_with_tokens(&format!("work-{index}"), /*total_tokens*/ 210_000),
        ]));
        bodies.push(sse(vec![
            ev_assistant_message(
                &format!("summary-{index}"),
                "Work remains. Continue the original task.",
            ),
            ev_completed_with_tokens(&format!("compact-{index}"), /*total_tokens*/ 100),
        ]));
    }
    bodies.push(sse(vec![
        ev_assistant_message("done", "All requested work is finished."),
        ev_completed_with_tokens("final", /*total_tokens*/ 200),
    ]));
    bodies.push(sse(vec![
        ev_assistant_message("follow-up", "The completed work remains available."),
        ev_completed_with_tokens("next-turn", /*total_tokens*/ 300),
    ]));
    let response = mount_sse_sequence(&server, bodies).await;
    let test = test_codex()
        .with_model(model)
        .with_config(|config| {
            config.model_provider.name = "Shared gateway".to_string();
            config.model_auto_compact_token_limit = Some(200_000);
            config.update_plan_enabled = true;
            config
                .features
                .disable(Feature::TokenBudget)
                .expect("disable token budget");
        })
        .build_with_auto_env(&server)
        .await?;
    test.submit_turn("Finish all requested work across context windows.")
        .await?;
    test.submit_turn("Check the result again.").await?;
    let requests = response.requests();
    assert_eq!(requests.len(), 6);
    for index in [0, 2, 4, 5] {
        let guidance = requests[index]
            .message_input_texts("developer")
            .into_iter()
            .filter(|text| text.contains("<context_continuation>"))
            .collect::<Vec<_>>();
        assert_eq!(
            guidance.len(),
            1,
            "one runtime policy before and after each compaction"
        );
        assert!(guidance[0].contains("Context-window capacity is not a task deadline"));
        assert!(guidance[0].contains("Do not mark an unfinished goal complete"));
        assert!(guidance[0].len() < 2_000);
    }
    assert_eq!(
        requests
            .iter()
            .map(|request| request.body_json()["model"].clone())
            .collect::<Vec<_>>(),
        vec![json!(model); 6]
    );
    Ok(())
}

#[test_case("deepseek-flash"; "deepseek")]
#[test_case("grok-4.5"; "grok")]
#[test_case("gpt-5.2"; "gpt")]
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn overflow_recovery_is_rearmed_after_successful_progress(model: &str) -> Result<()> {
    let server = start_mock_server().await;
    let mut bodies = Vec::new();
    for index in 0..2 {
        bodies.push(sse_failed(
            &format!("overflow-{index}"),
            "context_length_exceeded",
            "Context is full",
        ));
        bodies.push(sse(vec![
            ev_assistant_message(&format!("summary-{index}"), "Continue the unfinished task."),
            ev_completed_with_tokens(&format!("compact-{index}"), /*total_tokens*/ 100),
        ]));
        bodies.push(sse(vec![
            ev_function_call(
                &format!("plan-{index}"),
                "update_plan",
                &json!({"plan": [{"step": "Finish work", "status": "in_progress"}]}).to_string(),
            ),
            ev_completed_with_tokens(&format!("progress-{index}"), /*total_tokens*/ 200),
        ]));
    }
    bodies.push(sse(vec![
        ev_assistant_message("done", "Finished"),
        ev_completed_with_tokens("final", /*total_tokens*/ 300),
    ]));
    let response = mount_sse_sequence(&server, bodies).await;
    let test = test_codex()
        .with_model(model)
        .with_config(|config| {
            config.model_provider.name = "Shared gateway".to_string();
            config.update_plan_enabled = true;
            config
                .features
                .disable(Feature::TokenBudget)
                .expect("disable token budget");
        })
        .build_with_auto_env(&server)
        .await?;
    test.submit_turn("Keep working across multiple context windows.")
        .await?;
    assert_eq!(
        response.requests().len(),
        7,
        "each new overflow after successful work gets its own recovery"
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ineffective_overflow_recovery_still_stops() -> Result<()> {
    let server = start_mock_server().await;
    let overflow = sse_failed("overflow", "context_length_exceeded", "Context is full");
    let response = mount_sse_sequence(
        &server,
        vec![
            overflow.clone(),
            sse(vec![
                ev_assistant_message("summary", "Still too large"),
                ev_completed_with_tokens("compact", /*total_tokens*/ 100),
            ]),
            overflow,
        ],
    )
    .await;
    let test = test_codex()
        .with_config(|config| {
            config.model_provider.name = "Shared gateway".to_string();
            config
                .features
                .disable(Feature::TokenBudget)
                .expect("disable token budget");
        })
        .build_with_auto_env(&server)
        .await?;
    test.submit_turn("Finish the task.").await?;
    assert_eq!(
        response.requests().len(),
        3,
        "failed recovery must not loop"
    );
    Ok(())
}
