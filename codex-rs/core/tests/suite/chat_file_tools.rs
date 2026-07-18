use codex_config::config_toml::ChatFileToolMode;
use codex_core::config::Config;
use codex_features::Feature;
use codex_model_provider_info::WireApi;
use codex_protocol::models::PermissionProfile;
use codex_protocol::protocol::AskForApproval;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::ReviewDecision;
use codex_protocol::protocol::SandboxPolicy;
use codex_protocol::user_input::UserInput;
use codex_tools::ChatToolCallKind;
use codex_tools::chat_tool_name;
use core_test_support::responses::mount_chat_sse_sequence;
use core_test_support::responses::start_mock_server;
use core_test_support::test_codex::TestCodex;
use core_test_support::test_codex::local_selections;
use core_test_support::test_codex::test_codex;
use core_test_support::test_codex::turn_permission_fields;
use core_test_support::wait_for_event;
use pretty_assertions::assert_eq;
use serde_json::Value;
use serde_json::json;
use std::path::PathBuf;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tokio::io::AsyncWriteExt;
use wiremock::Mock;
use wiremock::Request;
use wiremock::Respond;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path_regex;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_dedicated_write_create_then_edit_succeeds_across_completions() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let write_name = chat_tool_name(None, "write_file", ChatToolCallKind::Function);
    let edit_name = chat_tool_name(None, "edit_file", ChatToolCallKind::Function);
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "create",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "write_1",
                    "function": {"name": write_name, "arguments": "{\"path\":\"created.txt\",\"content\":\"before\\n\"}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "edit",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "edit_1",
                    "function": {"name": edit_name, "arguments": "{\"path\":\"created.txt\",\"old_string\":\"before\",\"new_string\":\"after\"}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "final",
                "choices": [{"index": 0, "delta": {"content": "done"}, "finish_reason": "stop"}]
            })]),
        ],
    )
    .await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test.executor_environment().cwd().join("created.txt");

    submit_dedicated_turn(&test, "create and refine created.txt").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(tokio::fs::read(&target_path).await?, b"after\n");
    let requests = responses.requests();
    assert_eq!(requests.len(), 3);
    let final_messages = requests[2].body_json()["messages"]
        .as_array()
        .cloned()
        .expect("Chat messages");
    assert_tool_result_contains(&final_messages, "write_1", "created");
    assert_tool_result_contains(&final_messages, "edit_1", "completed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_dedicated_same_batch_create_cannot_authorize_edit() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let write_name = chat_tool_name(None, "write_file", ChatToolCallKind::Function);
    let edit_name = chat_tool_name(None, "edit_file", ChatToolCallKind::Function);
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "same_batch",
                "choices": [{"index": 0, "delta": {"tool_calls": [
                    {
                        "index": 0,
                        "id": "write_same_step",
                        "function": {"name": write_name, "arguments": "{\"path\":\"same-step.txt\",\"content\":\"before\\n\"}"}
                    },
                    {
                        "index": 1,
                        "id": "edit_same_step",
                        "function": {"name": edit_name, "arguments": "{\"path\":\"same-step.txt\",\"old_string\":\"before\",\"new_string\":\"after\"}"}
                    }
                ]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "final",
                "choices": [{"index": 0, "delta": {"content": "handled"}, "finish_reason": "stop"}]
            })]),
        ],
    )
    .await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test.executor_environment().cwd().join("same-step.txt");

    submit_dedicated_turn(&test, "try a same-batch create and edit").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(tokio::fs::read(&target_path).await?, b"before\n");
    let requests = responses.requests();
    assert_eq!(requests.len(), 2);
    let messages = requests[1].body_json()["messages"]
        .as_array()
        .cloned()
        .expect("Chat messages");
    assert_tool_result_contains(&messages, "write_same_step", "created");
    assert_tool_result_contains(&messages, "edit_same_step", "separate completions");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_dedicated_same_batch_read_cannot_authorize_edit() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let read_name = chat_tool_name(None, "read_file", ChatToolCallKind::Function);
    let edit_name = chat_tool_name(None, "edit_file", ChatToolCallKind::Function);
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "same_batch_read",
                "choices": [{"index": 0, "delta": {"tool_calls": [
                    {
                        "index": 0,
                        "id": "read_same_step",
                        "function": {"name": read_name, "arguments": "{\"path\":\"same-read.txt\"}"}
                    },
                    {
                        "index": 1,
                        "id": "edit_same_step",
                        "function": {"name": edit_name, "arguments": "{\"path\":\"same-read.txt\",\"old_string\":\"before\",\"new_string\":\"after\"}"}
                    }
                ]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "final",
                "choices": [{"index": 0, "delta": {"content": "handled"}, "finish_reason": "stop"}]
            })]),
        ],
    )
    .await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test.executor_environment().cwd().join("same-read.txt");
    tokio::fs::write(&target_path, b"before\n").await?;

    submit_dedicated_turn(&test, "try a same-batch read and edit").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(tokio::fs::read(&target_path).await?, b"before\n");
    let messages = responses.requests()[1].body_json()["messages"]
        .as_array()
        .cloned()
        .expect("Chat messages");
    assert_tool_result_contains(&messages, "edit_same_step", "separate completions");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_dedicated_same_batch_overwrites_do_not_chain_receipts() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let read_name = chat_tool_name(None, "read_file", ChatToolCallKind::Function);
    let edit_name = chat_tool_name(None, "edit_file", ChatToolCallKind::Function);
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "read",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "read_prior_step",
                    "function": {"name": read_name, "arguments": "{\"path\":\"double-edit.txt\"}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "double_edit",
                "choices": [{"index": 0, "delta": {"tool_calls": [
                    {
                        "index": 0,
                        "id": "edit_one",
                        "function": {"name": edit_name.clone(), "arguments": "{\"path\":\"double-edit.txt\",\"old_string\":\"before\",\"new_string\":\"after-one\"}"}
                    },
                    {
                        "index": 1,
                        "id": "edit_two",
                        "function": {"name": edit_name, "arguments": "{\"path\":\"double-edit.txt\",\"old_string\":\"before\",\"new_string\":\"after-two\"}"}
                    }
                ]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "final",
                "choices": [{"index": 0, "delta": {"content": "handled"}, "finish_reason": "stop"}]
            })]),
        ],
    )
    .await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test.executor_environment().cwd().join("double-edit.txt");
    tokio::fs::write(&target_path, b"before\n").await?;

    submit_dedicated_turn(&test, "try two same-batch overwrites").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let final_bytes = tokio::fs::read(&target_path).await?;
    assert!(final_bytes == b"after-one\n" || final_bytes == b"after-two\n");
    let messages = responses.requests()[2].body_json()["messages"]
        .as_array()
        .cloned()
        .expect("Chat messages");
    let results = ["edit_one", "edit_two"].map(|call_id| {
        messages
            .iter()
            .find(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
            .and_then(|message| message["content"].as_str())
            .unwrap_or_default()
    });
    assert_eq!(
        results
            .iter()
            .filter(|result| result.contains("completed"))
            .count(),
        1
    );
    assert!(
        results
            .iter()
            .any(|result| { result.contains("separate completions") })
    );
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_partial_receipt_allows_only_observed_edits_and_rejects_overwrite()
-> anyhow::Result<()> {
    let server = start_mock_server().await;
    let read_name = chat_tool_name(None, "read_file", ChatToolCallKind::Function);
    let edit_name = chat_tool_name(None, "edit_file", ChatToolCallKind::Function);
    let write_name = chat_tool_name(None, "write_file", ChatToolCallKind::Function);
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "partial_read",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "partial_read",
                    "function": {"name": read_name, "arguments": "{\"path\":\"partial.txt\",\"offset\":2,\"limit\":1}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "outside_edit",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "outside_edit",
                    "function": {"name": edit_name.clone(), "arguments": "{\"path\":\"partial.txt\",\"old_string\":\"one\",\"new_string\":\"ONE\"}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "partial_write",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "partial_write",
                    "function": {"name": write_name, "arguments": "{\"path\":\"partial.txt\",\"content\":\"replacement\\n\"}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "inside_edit",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "inside_edit",
                    "function": {"name": edit_name, "arguments": "{\"path\":\"partial.txt\",\"old_string\":\"two\",\"new_string\":\"TWO\"}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "final",
                "choices": [{"index": 0, "delta": {"content": "done"}, "finish_reason": "stop"}]
            })]),
        ],
    )
    .await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test.executor_environment().cwd().join("partial.txt");
    tokio::fs::write(&target_path, b"one\ntwo\nthree\n").await?;

    submit_dedicated_turn(&test, "respect a partial read receipt").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(tokio::fs::read(&target_path).await?, b"one\nTWO\nthree\n");
    let final_messages = responses.requests()[4].body_json()["messages"]
        .as_array()
        .cloned()
        .expect("Chat messages");
    assert_tool_result_contains(
        &final_messages,
        "outside_edit",
        "outside the observed read range",
    );
    assert_tool_result_contains(
        &final_messages,
        "partial_write",
        "complete read is required",
    );
    assert_tool_result_contains(&final_messages, "inside_edit", "completed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_dedicated_missing_receipt_is_correctable_after_read() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let read_name = chat_tool_name(None, "read_file", ChatToolCallKind::Function);
    let edit_name = chat_tool_name(None, "edit_file", ChatToolCallKind::Function);
    let edit_arguments =
        "{\"path\":\"missing-receipt.txt\",\"old_string\":\"before\",\"new_string\":\"after\"}";
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "missing",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "edit_without_read",
                    "function": {"name": edit_name.clone(), "arguments": edit_arguments}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "read",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "read_after_failure",
                    "function": {"name": read_name, "arguments": "{\"path\":\"missing-receipt.txt\"}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "retry_edit",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "edit_after_read",
                    "function": {"name": edit_name, "arguments": edit_arguments}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "final",
                "choices": [{"index": 0, "delta": {"content": "recovered"}, "finish_reason": "stop"}]
            })]),
        ],
    )
    .await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test
        .executor_environment()
        .cwd()
        .join("missing-receipt.txt");
    test.fs()
        .write_file(
            &codex_utils_path_uri::PathUri::from_abs_path(&target_path),
            b"before\n".to_vec(),
            None,
        )
        .await?;

    test.submit_turn("recover from a missing receipt").await?;

    assert_eq!(
        test.fs()
            .read_file(
                &codex_utils_path_uri::PathUri::from_abs_path(&target_path),
                None,
            )
            .await?,
        b"after\n"
    );
    let requests = responses.requests();
    assert_eq!(requests.len(), 4);
    let messages_after_failure = requests[1].body_json()["messages"]
        .as_array()
        .cloned()
        .expect("Chat messages");
    assert_tool_result_contains(
        &messages_after_failure,
        "edit_without_read",
        "read_file is required",
    );
    let final_messages = requests[3].body_json()["messages"]
        .as_array()
        .cloned()
        .expect("Chat messages");
    assert_tool_result_contains(&final_messages, "edit_after_read", "completed");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_dedicated_external_change_after_read_rejects_stale_edit() -> anyhow::Result<()> {
    struct MutatingSequence {
        call_count: AtomicUsize,
        target_path: PathBuf,
        responses: Vec<String>,
    }

    impl Respond for MutatingSequence {
        fn respond(&self, _request: &Request) -> ResponseTemplate {
            let call = self.call_count.fetch_add(1, Ordering::SeqCst);
            if call == 1 {
                std::fs::write(&self.target_path, b"external\n")
                    .expect("external modification should succeed");
            }
            ResponseTemplate::new(200)
                .insert_header("content-type", "text/event-stream")
                .set_body_string(
                    self.responses
                        .get(call)
                        .unwrap_or_else(|| panic!("missing response {call}"))
                        .clone(),
                )
        }
    }

    let server = start_mock_server().await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test.executor_environment().cwd().join("stale-edit.txt");
    tokio::fs::write(&target_path, b"before\n").await?;
    let read_name = chat_tool_name(None, "read_file", ChatToolCallKind::Function);
    let edit_name = chat_tool_name(None, "edit_file", ChatToolCallKind::Function);
    let responses = vec![
        chat_sse(vec![json!({
            "id": "read",
            "choices": [{"index": 0, "delta": {"tool_calls": [{
                "index": 0,
                "id": "read_stale",
                "function": {"name": read_name, "arguments": "{\"path\":\"stale-edit.txt\"}"}
            }]}, "finish_reason": "tool_calls"}]
        })]),
        chat_sse(vec![json!({
            "id": "edit",
            "choices": [{"index": 0, "delta": {"tool_calls": [{
                "index": 0,
                "id": "edit_stale",
                "function": {"name": edit_name, "arguments": "{\"path\":\"stale-edit.txt\",\"old_string\":\"before\",\"new_string\":\"after\"}"}
            }]}, "finish_reason": "tool_calls"}]
        })]),
        chat_sse(vec![json!({
            "id": "final",
            "choices": [{"index": 0, "delta": {"content": "handled"}, "finish_reason": "stop"}]
        })]),
    ];
    Mock::given(method("POST"))
        .and(path_regex(".*/chat/completions$"))
        .respond_with(MutatingSequence {
            call_count: AtomicUsize::new(0),
            target_path: target_path.to_path_buf(),
            responses,
        })
        .expect(3)
        .mount(&server)
        .await;

    submit_dedicated_turn(&test, "reject a stale edit").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(tokio::fs::read(&target_path).await?, b"external\n");
    let requests = server
        .received_requests()
        .await
        .expect("requests should be captured");
    let final_request: Value = serde_json::from_slice(&requests[2].body)?;
    let messages = final_request["messages"].as_array().expect("Chat messages");
    assert_tool_result_contains(messages, "edit_stale", "read it again");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_dedicated_partial_read_is_incomplete_without_false_truncation() -> anyhow::Result<()>
{
    let server = start_mock_server().await;
    let read_name = chat_tool_name(None, "read_file", ChatToolCallKind::Function);
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "read",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "partial_read",
                    "function": {"name": read_name, "arguments": "{\"path\":\"partial.txt\",\"offset\":2,\"limit\":1}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "final",
                "choices": [{"index": 0, "delta": {"content": "done"}, "finish_reason": "stop"}]
            })]),
        ],
    )
    .await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test.executor_environment().cwd().join("partial.txt");
    tokio::fs::write(target_path, b"one\ntwo\nthree\n").await?;

    submit_dedicated_turn(&test, "read the middle line").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let requests = responses.requests();
    let request = requests[1].body_json();
    let messages = request["messages"].as_array().expect("Chat messages");
    let result = tool_result_text(messages, "partial_read");
    assert!(result.contains("Lines: 2-2"));
    assert!(result.contains("Complete: false"));
    assert!(!result.contains("output truncated"));
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_dedicated_edit_rejects_files_above_editable_cap() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let read_name = chat_tool_name(None, "read_file", ChatToolCallKind::Function);
    let edit_name = chat_tool_name(None, "edit_file", ChatToolCallKind::Function);
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "read",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "large_read",
                    "function": {"name": read_name, "arguments": "{\"path\":\"large.txt\",\"limit\":1}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "edit",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "large_edit",
                    "function": {"name": edit_name, "arguments": "{\"path\":\"large.txt\",\"old_string\":\"a\",\"new_string\":\"b\"}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "final",
                "choices": [{"index": 0, "delta": {"content": "handled"}, "finish_reason": "stop"}]
            })]),
        ],
    )
    .await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test.executor_environment().cwd().join("large.txt");
    let original = vec![b'a'; 8 * 1024 * 1024 + 1];
    tokio::fs::write(&target_path, &original).await?;

    submit_dedicated_turn(&test, "do not edit an oversized file").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(tokio::fs::read(&target_path).await?, original);
    let requests = responses.requests();
    let request = requests[2].body_json();
    let messages = request["messages"].as_array().expect("Chat messages");
    assert_tool_result_contains(messages, "large_edit", "8 MiB editable limit");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_dedicated_large_range_read_stops_at_scan_cap_with_unknown_total() -> anyhow::Result<()>
{
    let server = start_mock_server().await;
    let read_name = chat_tool_name(None, "read_file", ChatToolCallKind::Function);
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "read",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "bounded_large_read",
                    "function": {"name": read_name, "arguments": "{\"path\":\"scan-cap.txt\",\"limit\":1}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "final",
                "choices": [{"index": 0, "delta": {"content": "done"}, "finish_reason": "stop"}]
            })]),
        ],
    )
    .await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test.executor_environment().cwd().join("scan-cap.txt");
    let mut file = tokio::fs::File::create(&target_path).await?;
    file.write_all(b"first\n").await?;
    let chunk = vec![b'a'; 1024 * 1024];
    for _ in 0..64 {
        file.write_all(&chunk).await?;
    }
    file.write_all(b"tail\n").await?;
    file.flush().await?;
    drop(file);

    submit_dedicated_turn(&test, "read the first line of the large file").await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    let request = responses.requests()[1].body_json();
    let messages = request["messages"].as_array().expect("Chat messages");
    let result = tool_result_text(messages, "bounded_large_read");
    assert!(result.contains("Lines: 1-1"), "{result}");
    assert!(result.contains("Total lines: unknown"), "{result}");
    assert!(result.contains("Complete: false"), "{result}");
    assert!(result.contains("Write eligible: false"), "{result}");
    assert!(result.contains("L1: first"), "{result}");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_dedicated_approval_pending_edit_rejects_external_change() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let read_name = chat_tool_name(None, "read_file", ChatToolCallKind::Function);
    let edit_name = chat_tool_name(None, "edit_file", ChatToolCallKind::Function);
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "read",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "approval_read",
                    "function": {"name": read_name, "arguments": "{\"path\":\"approval-stale.txt\"}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "edit",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "approval_edit",
                    "function": {"name": edit_name, "arguments": "{\"path\":\"approval-stale.txt\",\"old_string\":\"before\",\"new_string\":\"after\"}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "final",
                "choices": [{"index": 0, "delta": {"content": "handled"}, "finish_reason": "stop"}]
            })]),
        ],
    )
    .await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test.executor_environment().cwd().join("approval-stale.txt");
    tokio::fs::write(&target_path, b"before\n").await?;

    submit_dedicated_turn_with_approval(&test, "edit after approval").await?;
    let approval = wait_for_event(&test.codex, |event| {
        matches!(
            event,
            EventMsg::ApplyPatchApprovalRequest(_) | EventMsg::TurnComplete(_)
        )
    })
    .await;
    let EventMsg::ApplyPatchApprovalRequest(approval) = approval else {
        panic!("expected an edit approval request");
    };
    assert_eq!(approval.call_id, "approval_edit");
    tokio::fs::write(&target_path, b"external\n").await?;
    test.codex
        .submit(Op::PatchApproval {
            id: approval.call_id,
            decision: ReviewDecision::Approved,
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(tokio::fs::read(&target_path).await?, b"external\n");
    let requests = responses.requests();
    let request = requests[2].body_json();
    let messages = request["messages"].as_array().expect("Chat messages");
    assert_tool_result_contains(messages, "approval_edit", "conflict");
    Ok(())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn chat_dedicated_approval_pending_create_rejects_appeared_target() -> anyhow::Result<()> {
    let server = start_mock_server().await;
    let write_name = chat_tool_name(None, "write_file", ChatToolCallKind::Function);
    let responses = mount_chat_sse_sequence(
        &server,
        vec![
            chat_sse(vec![json!({
                "id": "create",
                "choices": [{"index": 0, "delta": {"tool_calls": [{
                    "index": 0,
                    "id": "approval_create",
                    "function": {"name": write_name, "arguments": "{\"path\":\"approval-create.txt\",\"content\":\"model\\n\"}"}
                }]}, "finish_reason": "tool_calls"}]
            })]),
            chat_sse(vec![json!({
                "id": "final",
                "choices": [{"index": 0, "delta": {"content": "handled"}, "finish_reason": "stop"}]
            })]),
        ],
    )
    .await;
    let test = build_dedicated_chat(&server).await?;
    let target_path = test
        .executor_environment()
        .cwd()
        .join("approval-create.txt");

    submit_dedicated_turn_with_approval(&test, "create after approval").await?;
    let approval = wait_for_event(&test.codex, |event| {
        matches!(
            event,
            EventMsg::ApplyPatchApprovalRequest(_) | EventMsg::TurnComplete(_)
        )
    })
    .await;
    let EventMsg::ApplyPatchApprovalRequest(approval) = approval else {
        panic!("expected a create approval request");
    };
    assert_eq!(approval.call_id, "approval_create");
    tokio::fs::write(&target_path, b"external\n").await?;
    test.codex
        .submit(Op::PatchApproval {
            id: approval.call_id,
            decision: ReviewDecision::Approved,
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;

    assert_eq!(tokio::fs::read(&target_path).await?, b"external\n");
    let requests = responses.requests();
    let request = requests[1].body_json();
    let messages = request["messages"].as_array().expect("Chat messages");
    assert_tool_result_contains(messages, "approval_create", "conflict");
    Ok(())
}

async fn build_dedicated_chat(server: &wiremock::MockServer) -> anyhow::Result<TestCodex> {
    test_codex()
        .with_model("gpt-5.2")
        .with_config(configure_dedicated_chat_provider)
        .build_with_auto_env(server)
        .await
}

async fn submit_dedicated_turn(test: &TestCodex, text: &str) -> anyhow::Result<()> {
    let (sandbox_policy, permission_profile) =
        turn_permission_fields(PermissionProfile::Disabled, test.config.cwd.as_path());
    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: text.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: codex_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(AskForApproval::Never),
                sandbox_policy: Some(sandbox_policy),
                permission_profile,
                ..Default::default()
            },
        })
        .await?;
    Ok(())
}

async fn submit_dedicated_turn_with_approval(test: &TestCodex, text: &str) -> anyhow::Result<()> {
    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: text.to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: codex_protocol::protocol::ThreadSettingsOverrides {
                environments: Some(local_selections(test.config.cwd.clone())),
                approval_policy: Some(AskForApproval::OnRequest),
                sandbox_policy: Some(SandboxPolicy::new_read_only_policy()),
                ..Default::default()
            },
        })
        .await?;
    Ok(())
}

fn configure_dedicated_chat_provider(config: &mut Config) {
    config.model_provider.name = "Chat Completions".to_string();
    config.model_provider.env_key = None;
    config.model_provider.experimental_bearer_token = Some("test-token".to_string());
    config.model_provider.requires_openai_auth = false;
    config.model_provider.supports_websockets = true;
    config.model_provider.stream_max_retries = Some(0);
    config.model_provider.wire_api = WireApi::Chat;
    config.chat_file_tool_mode = ChatFileToolMode::Dedicated;
    config.workspace_roots = vec![config.cwd.clone()];
    config
        .permissions
        .set_workspace_roots(config.workspace_roots.clone());
    config
        .features
        .enable(Feature::DedicatedFileTools)
        .expect("dedicated file-tools gate should be enableable");
}

fn assert_tool_result_contains(messages: &[Value], call_id: &str, expected: &str) {
    let result = tool_result_text(messages, call_id);
    assert!(
        result.contains(expected),
        "unexpected result for {call_id}: {result:?}"
    );
}

fn tool_result_text<'a>(messages: &'a [Value], call_id: &str) -> &'a str {
    messages
        .iter()
        .find(|message| message["role"] == "tool" && message["tool_call_id"] == call_id)
        .unwrap_or_else(|| panic!("missing tool result {call_id}: {messages:?}"))["content"]
        .as_str()
        .unwrap_or_else(|| panic!("tool result {call_id} should contain text"))
}

fn chat_sse(events: Vec<Value>) -> String {
    let mut body = events
        .into_iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    body.push_str("data: [DONE]\n\n");
    body
}
