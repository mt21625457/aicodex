use super::*;
use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use pretty_assertions::assert_eq;
use serde_json::json;

#[tokio::test]
async fn typed_provider_requests_send_prepared_history_and_tool_images_once() -> Result<()> {
    for model in [
        "deepseek-flash",
        "gpt-6",
        "mimo-v2.6-pro",
        "MiniMax-M3",
        "mimo:mimo-v2.6-flash",
        "minimax:MiniMax-M3",
    ] {
        let state = RecordingState::default();
        let mut settings = provider("gateway");
        settings.request_body_max_bytes = Some(4096);
        let client = ResponsesClient::new(
            RecordingTransport::new(state.clone()),
            settings,
            Arc::new(NoAuth),
        );
        let mut png = STANDARD.decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAIAAACQd1PeAAAADElEQVR4nGMQMgkDAAD4AJ3MaiF4AAAAAElFTkSuQmCC")?;
        png.resize(2 * 1024 * 1024, 0);
        let image = json!({"type":"input_image", "image_url":format!("data:image/png;base64,{}", STANDARD.encode(png))});
        let input = json!([
            {"type":"message", "role":"user", "content":[image.clone()]},
            {"type":"function_call_output", "call_id":"screenshot", "output":[image]},
            {"type":"message", "role":"user", "content":[{"type":"input_text", "text":"continue"}]}
        ]);
        let request = ResponsesApiRequest {
            model: model.into(),
            instructions: String::new(),
            input: serde_json::from_value(input)?,
            tools: Some(empty_tools().into()),
            tool_choice: "auto".into(),
            parallel_tool_calls: false,
            reasoning: None,
            max_output_tokens: None,
            store: false,
            stream: true,
            stream_options: None,
            include: Vec::new(),
            service_tier: None,
            prompt_cache_key: None,
            text: None,
            client_metadata: None,
            access_programs: None,
        };
        let original = serde_json::to_value(&request)?;
        let _stream = client
            .stream_request(request.clone(), ResponsesOptions::default())
            .await?;
        let sent = state.take_stream_requests();
        assert_eq!(sent.len(), 1);
        let bytes = request_body_bytes(&sent[0]);
        assert!(bytes.len() < 4096);
        let body: serde_json::Value = serde_json::from_slice(bytes)?;
        let mut expected = original.clone();
        expected["input"][0]["content"][0]["image_url"] =
            body["input"][0]["content"][0]["image_url"].clone();
        expected["input"][1]["output"][0]["image_url"] =
            body["input"][0]["content"][0]["image_url"].clone();
        assert_eq!(body, expected);
        assert_eq!(serde_json::to_value(request)?, original);
    }
    Ok(())
}

#[tokio::test]
async fn oversized_request_never_reaches_http_even_with_compression_enabled() -> Result<()> {
    for compression in [Compression::None, Compression::Zstd] {
        let state = RecordingState::default();
        let mut settings = provider("gateway");
        settings.retry.max_attempts = 3;
        let client = ResponsesClient::new(
            RecordingTransport::new(state.clone()),
            settings,
            Arc::new(NoAuth),
        );
        let body = json!({"model":"deepseek-flash", "input":[], "instructions":"x".repeat(40 * 1024 * 1024)});
        let error = match client
            .stream(
                body,
                HeaderMap::new(),
                compression,
                /*turn_state*/ None,
            )
            .await
        {
            Ok(_) => panic!("oversized request must fail"),
            Err(error) => error,
        };
        assert!(!codex_api::map_api_error(error).is_retryable());
        assert!(state.take_stream_requests().is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn upstream_413_is_not_retried_or_reclassified_as_transient() -> Result<()> {
    #[derive(Clone)]
    struct RejectingTransport(RecordingState);
    impl HttpTransport for RejectingTransport {
        async fn execute(&self, _: Request) -> Result<Response, TransportError> {
            unreachable!()
        }
        async fn stream(&self, request: Request) -> Result<StreamResponse, TransportError> {
            self.0.record_stream(request);
            Err(TransportError::Http {
                status: StatusCode::PAYLOAD_TOO_LARGE,
                url: None,
                headers: None,
                body: Some("Failed to buffer the request body: length limit exceeded".into()),
            })
        }
    }
    let state = RecordingState::default();
    let mut settings = provider("gateway");
    settings.retry.max_attempts = 3;
    settings.retry.retry_transport = true;
    settings.retry.retry_5xx = true;
    let client = ResponsesClient::new(
        RejectingTransport(state.clone()),
        settings,
        Arc::new(NoAuth),
    );
    let error = match client
        .stream(
            json!({"model":"deepseek-flash", "input":[]}),
            HeaderMap::new(),
            Compression::None,
            /*turn_state*/ None,
        )
        .await
    {
        Ok(_) => panic!("gateway rejects request"),
        Err(error) => error,
    };
    assert_eq!(state.take_stream_requests().len(), 1);
    let error = codex_api::map_api_error(error);
    assert!(!error.is_retryable());
    assert!(error.to_string().contains("length limit exceeded"));
    Ok(())
}
