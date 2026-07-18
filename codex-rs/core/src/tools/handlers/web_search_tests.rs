use super::*;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::WireApi;
use codex_model_provider_info::create_oss_provider_with_base_url;
use codex_protocol::config_types::WebSearchContextSize as ConfigWebSearchContextSize;
use codex_tools::ResponsesApiWebSearchFilters;
use codex_tools::ResponsesApiWebSearchUserLocation;
use pretty_assertions::assert_eq;
use serde_json::json;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;

use crate::session::tests::make_session_and_context;
use crate::tools::context::ToolCallSource;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::turn_diff_tracker::TurnDiffTracker;
use codex_model_provider::create_model_provider;

#[test]
fn parse_search_commands_accepts_openai_commands_and_legacy_aliases() {
    let commands = parse_search_commands(
        r#"{
            "query": " rust async ",
            "queries": ["tokio runtime", "rust async", " "],
            "search_query": [{"q": "existing query", "recency": 7}],
            "open": [{"ref_id": "https://example.com/docs", "lineno": 12}]
        }"#,
    )
    .expect("commands should parse");

    assert_eq!(
        commands.search_query.as_ref().expect("search queries"),
        &vec![
            SearchQuery {
                q: "rust async".to_string(),
                recency: None,
                domains: None,
            },
            SearchQuery {
                q: "tokio runtime".to_string(),
                recency: None,
                domains: None,
            },
            SearchQuery {
                q: "existing query".to_string(),
                recency: Some(7),
                domains: None,
            },
        ]
    );
    assert_eq!(
        command_action(&commands),
        WebSearchAction::Search {
            query: None,
            queries: Some(vec![
                "rust async".to_string(),
                "tokio runtime".to_string(),
                "existing query".to_string()
            ]),
        }
    );
}

#[test]
fn search_settings_preserve_openai_web_search_options() {
    let spec = ToolSpec::WebSearch {
        external_web_access: Some(true),
        indexed_web_access: Some(true),
        filters: Some(ResponsesApiWebSearchFilters {
            allowed_domains: Some(vec![
                "example.com".to_string(),
                "docs.example.com".to_string(),
            ]),
        }),
        user_location: Some(ResponsesApiWebSearchUserLocation {
            r#type: codex_protocol::config_types::WebSearchUserLocationType::Approximate,
            country: Some("US".to_string()),
            region: Some("California".to_string()),
            city: Some("San Francisco".to_string()),
            timezone: Some("America/Los_Angeles".to_string()),
        }),
        search_context_size: Some(ConfigWebSearchContextSize::High),
        search_content_types: Some(vec!["text".to_string(), "image".to_string()]),
    };

    let settings = search_settings_from_spec(&spec);

    assert_eq!(settings.search_context_size, Some(SearchContextSize::High));
    assert_eq!(
        settings.filters,
        Some(SearchFilters {
            allowed_domains: Some(vec![
                "example.com".to_string(),
                "docs.example.com".to_string()
            ]),
            blocked_domains: None,
        })
    );
    assert_eq!(
        settings.external_web_access,
        Some(ExternalWebAccess::Mode(ExternalWebAccessMode::Indexed))
    );
    assert_eq!(
        settings.user_location,
        Some(ApproximateLocation {
            r#type: LocationType::Approximate,
            country: Some("US".to_string()),
            region: Some("California".to_string()),
            city: Some("San Francisco".to_string()),
            timezone: Some("America/Los_Angeles".to_string()),
        })
    );
}

#[tokio::test]
async fn handler_calls_openai_search_endpoint_and_returns_search_output() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/alpha/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "output": "search result from OpenAI search"
        })))
        .expect(1)
        .mount(&server)
        .await;

    let spec = ToolSpec::WebSearch {
        external_web_access: Some(true),
        indexed_web_access: None,
        filters: Some(ResponsesApiWebSearchFilters {
            allowed_domains: Some(vec!["example.com".to_string()]),
        }),
        user_location: None,
        search_context_size: Some(ConfigWebSearchContextSize::Low),
        search_content_types: None,
    };
    let mut provider =
        ModelProviderInfo::create_openai_provider(Some(format!("{}/v1", server.uri())));
    provider.requires_openai_auth = false;
    provider.http_headers = None;
    let handler = WebSearchHandler::new_for_test(spec, provider, "gpt-search-test");

    let output = handler
        .handle(invocation_for_arguments(r#"{"search_query":[{"q":"rust async"}]}"#).await)
        .await
        .expect("web search should succeed");

    assert_eq!(output.log_preview(), "[openai web search output]");
    assert!(output.contains_external_context());
    let response = output.to_response_item(
        "call-web-search",
        &ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    );
    assert_eq!(
        response,
        ResponseInputItem::FunctionCallOutput {
            call_id: "call-web-search".to_string(),
            output: FunctionCallOutputPayload::from_content_items(vec![
                FunctionCallOutputContentItem::InputText {
                    text: "search result from OpenAI search".to_string(),
                },
            ]),
        }
    );

    let requests = server
        .received_requests()
        .await
        .expect("mock server should capture requests");
    let body: serde_json::Value = requests[0]
        .body_json()
        .expect("request body should be valid json");
    assert_eq!(body["model"], json!("gpt-search-test"));
    assert_eq!(
        body["commands"]["search_query"][0]["q"],
        json!("rust async")
    );
    assert_eq!(
        body["settings"]["filters"]["allowed_domains"],
        json!(["example.com"])
    );
    assert_eq!(body["settings"]["search_context_size"], json!("low"));
    assert!(
        body["max_output_tokens"]
            .as_u64()
            .is_some_and(|value| value > 0)
    );
}

#[tokio::test]
async fn kimi_handler_calls_moonshot_and_returns_bounded_external_context() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "search_results": [{
                "title": "Rust 1.90",
                "url": "https://blog.rust-lang.org/release",
                "snippet": "Rust 1.90 is available.",
                "content": "full page must not enter context"
            }]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let spec = ToolSpec::WebSearch {
        external_web_access: Some(true),
        indexed_web_access: None,
        filters: None,
        user_location: None,
        search_context_size: None,
        search_content_types: None,
    };
    let handler = WebSearchHandler::new(spec);
    let (session, mut turn) = make_session_and_context().await;
    let mut config = (*turn.config).clone();
    config
        .features
        .enable(codex_features::Feature::KimiMoonshotWebSearch)
        .expect("test feature should enable");
    config.moonshot_search.base_url = Some(format!("{}/v1/search", server.uri()));
    config.moonshot_search.api_key = Some("moonshot-test-token".to_string());
    turn.config = Arc::new(config);
    turn.model_info.slug = "gateway:k3".to_string();
    let mut provider =
        create_oss_provider_with_base_url(&format!("{}/v1", server.uri()), WireApi::Claude);
    provider.experimental_bearer_token = Some("provider-token".to_string());
    turn.provider = create_model_provider(provider, /*auth_manager*/ None);
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let step_context = session.capture_step_context(Arc::clone(&turn)).await;
    let output = handler
        .handle(ToolInvocation {
            session,
            turn,
            step_context,
            cancellation_token: CancellationToken::new(),
            tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
            call_id: "call-moonshot".to_string(),
            tool_name: codex_tools::ToolName::plain("web_search"),
            source: ToolCallSource::Direct,
            payload: ToolPayload::Function {
                arguments: r#"{"query":"latest rust release"}"#.to_string(),
            },
        })
        .await
        .expect("Moonshot search should succeed");

    assert_eq!(output.log_preview(), "[moonshot web search output]");
    assert!(output.contains_external_context());
    let response = output.to_response_item(
        "call-moonshot",
        &ToolPayload::Function {
            arguments: "{}".to_string(),
        },
    );
    let ResponseInputItem::FunctionCallOutput { output, .. } = response else {
        panic!("expected function call output");
    };
    let text = output
        .content_items()
        .and_then(|items| items.first())
        .and_then(|item| match item {
            FunctionCallOutputContentItem::InputText { text } => Some(text.as_str()),
            _ => None,
        })
        .expect("text output");
    assert!(text.contains("Rust 1.90 is available."));
    assert!(text.contains("https://blog.rust-lang.org/release"));
    assert!(!text.contains("full page must not enter context"));

    let requests = server
        .received_requests()
        .await
        .expect("mock server should capture request");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0].url.path(), "/v1/search");
    assert_eq!(
        requests[0]
            .headers
            .get(codex_api::MOONSHOT_TOOL_CALL_ID_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("call-moonshot")
    );
}

#[test]
fn command_action_reports_queries_and_navigation_detail() {
    let cases = [
        (
            r#"{"image_query":[{"q":"waterfalls"},{"q":"mountains"}]}"#,
            WebSearchAction::Search {
                query: None,
                queries: Some(vec!["waterfalls".to_string(), "mountains".to_string()]),
            },
        ),
        (
            r#"{"open":[{"ref_id":"https://example.com/docs"}]}"#,
            WebSearchAction::OpenPage {
                url: Some("https://example.com/docs".to_string()),
            },
        ),
        (
            r#"{"find":[{"ref_id":"https://example.com/docs","pattern":"install"}]}"#,
            WebSearchAction::FindInPage {
                url: Some("https://example.com/docs".to_string()),
                pattern: Some("install".to_string()),
            },
        ),
        (
            r#"{"find":[{"ref_id":"turn0search0","pattern":"install"}]}"#,
            WebSearchAction::FindInPage {
                url: None,
                pattern: Some("install".to_string()),
            },
        ),
        (
            r#"{"open":[{"ref_id":"turn0search0"}]}"#,
            WebSearchAction::Other,
        ),
    ];

    for (arguments, expected) in cases {
        let commands = parse_search_commands(arguments).expect("valid search command arguments");
        assert_eq!(command_action(&commands), expected);
    }
}

async fn invocation_for_arguments(arguments: &str) -> ToolInvocation {
    let (session, turn) = make_session_and_context().await;
    let session = Arc::new(session);
    let turn = Arc::new(turn);
    let step_context = session.capture_step_context(Arc::clone(&turn)).await;
    ToolInvocation {
        session,
        turn,
        step_context,
        cancellation_token: CancellationToken::new(),
        tracker: Arc::new(Mutex::new(TurnDiffTracker::new())),
        call_id: "call-web-search".to_string(),
        tool_name: codex_tools::ToolName::plain("web_search"),
        source: ToolCallSource::Direct,
        payload: ToolPayload::Function {
            arguments: arguments.to_string(),
        },
    }
}
