//! 自动压缩最终失败也必须通知目标等生命周期订阅者，避免空闲续跑。
use super::*;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::TurnLifecycleContributor;
use pretty_assertions::assert_eq;
use std::sync::Mutex;

struct ErrorRecorder(Arc<Mutex<Vec<CodexErrorInfo>>>);

impl TurnLifecycleContributor for ErrorRecorder {
    fn on_turn_error<'a>(
        &'a self,
        input: codex_extension_api::TurnErrorInput<'a>,
    ) -> codex_extension_api::ExtensionFuture<'a, ()> {
        Box::pin(async move {
            self.0
                .lock()
                .expect("error recorder lock")
                .push(input.error);
        })
    }
}

#[test]
fn admission_compaction_forbidden_notifies_terminal_lifecycle() -> Result<()> {
    // 与既有 admission fixture 一致，完整 turn future 需要较大的测试线程栈。
    let handle = std::thread::Builder::new()
        .name("admission_compaction_terminal_error".to_string())
        .stack_size(8 * 1024 * 1024)
        .spawn(|| -> Result<()> {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .thread_stack_size(8 * 1024 * 1024)
                .enable_all()
                .build()?
                .block_on(Box::pin(run_admission_compaction_failure()))
        })?;
    handle
        .join()
        .map_err(|_| anyhow!("admission compaction fixture panicked"))?
}

async fn run_admission_compaction_failure() -> Result<()> {
    let server = start_mock_server().await;
    // 与 admission 测试相同：工具名导致本地输出估算超过阈值，服务端用量仍低于阈值。
    let large_tool_name = format!("unsupported_{}", "tool_payload_".repeat(120));
    let requests = mount_response_sequence(
        &server,
        vec![
            sse_response(sse(vec![
                ev_function_call("call-admission", &large_tool_name, "{}"),
                ev_completed_with_tokens("r1", 60),
            ])),
            wiremock::ResponseTemplate::new(403).set_body_json(
                json!({"error": {"code": "permission_denied", "message": "fixture rejection"}}),
            ),
        ],
    )
    .await;
    let errors = Arc::new(Mutex::new(Vec::new()));
    let mut extensions = ExtensionRegistryBuilder::<Config>::new();
    extensions.turn_lifecycle_contributor(Arc::new(ErrorRecorder(Arc::clone(&errors))));
    let provider = non_openai_model_provider(&server);
    let test = test_codex()
        .with_extensions(Arc::new(extensions.build()))
        .with_config(move |config| {
            config.model_provider = provider;
            set_test_compact_prompt(config);
            config.model_auto_compact_token_limit = Some(300);
            config.model_auto_compact_token_limit_scope =
                AutoCompactTokenLimitScope::BodyAfterPrefix;
        })
        .build(&server)
        .await?;
    test.codex
        .submit(Op::UserInput {
            items: vec![UserInput::Text {
                text: "trigger a large unsupported tool call".to_string(),
                text_elements: Vec::new(),
            }],
            final_output_json_schema: None,
            responsesapi_client_metadata: None,
            additional_context: Default::default(),
            thread_settings: Default::default(),
        })
        .await?;
    wait_for_event(&test.codex, |event| {
        matches!(event, EventMsg::TurnComplete(_))
    })
    .await;
    assert_eq!(requests.requests().len(), 2);
    assert_eq!(
        *errors.lock().expect("error recorder lock"),
        vec![CodexErrorInfo::HttpConnectionFailed {
            http_status_code: Some(403)
        },]
    );
    Ok(())
}
