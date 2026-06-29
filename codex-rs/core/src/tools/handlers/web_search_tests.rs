use super::*;
use codex_protocol::config_types::WebSearchContextSize;
use codex_tools::ResponsesApiWebSearchFilters;
use pretty_assertions::assert_eq;
use std::sync::Arc;
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::matchers::query_param;

use crate::session::tests::make_session_and_context;
use crate::tools::context::ToolCallSource;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolPayload;
use crate::turn_diff_tracker::TurnDiffTracker;

#[test]
fn normalize_queries_accepts_single_and_batch_queries() {
    let queries = normalize_queries(WebSearchArgs {
        query: Some(" rust async ".to_string()),
        queries: Some(vec![
            "tokio runtime".to_string(),
            "rust async".to_string(),
            " ".to_string(),
        ]),
    })
    .expect("queries should normalize");

    assert_eq!(queries, vec!["rust async", "tokio runtime"]);
}

#[test]
fn parse_duckduckgo_results_decodes_links_titles_and_snippets() {
    let html = r#"
        <div class="result">
          <a rel="nofollow" class="result__a" href="//duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Frust%3Fa%3D1&amp;rut=abc">
            Rust &amp; async <b>guide</b>
          </a>
          <a class="result__snippet">Learn &lt;async&gt; Rust with examples.</a>
        </div>
    "#;

    assert_eq!(
        parse_duckduckgo_results(html, 5),
        vec![WebSearchResult {
            title: "Rust & async guide".to_string(),
            url: "https://example.com/rust?a=1".to_string(),
            snippet: "Learn <async> Rust with examples.".to_string(),
        }]
    );
}

#[test]
fn query_with_allowed_domains_adds_site_filter() {
    assert_eq!(
        query_with_allowed_domains(
            "rust async",
            &["example.com".to_string(), "docs.example.com".to_string()]
        ),
        "rust async (site:example.com OR site:docs.example.com)"
    );
}

#[tokio::test]
async fn run_web_searches_uses_configured_endpoint_and_returns_results() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/html/"))
        .and(query_param("q", "rust async (site:example.com)"))
        .respond_with(ResponseTemplate::new(200).set_body_string(
            r#"
                <div class="result">
                  <a class="result__a" href="https://example.com/rust">Rust async</a>
                  <div class="result__snippet">Async Rust result.</div>
                </div>
            "#,
        ))
        .expect(1)
        .mount(&server)
        .await;

    let endpoint = Url::parse(&format!("{}/html/", server.uri())).expect("mock endpoint");
    let response = run_web_searches(
        &reqwest::Client::new(),
        &endpoint,
        vec!["rust async".to_string()],
        &["example.com".to_string()],
        DEFAULT_RESULTS_PER_QUERY,
    )
    .await
    .expect("web search should succeed");

    assert_eq!(response.source, "duckduckgo_html");
    assert_eq!(
        response.searches[0].results,
        vec![WebSearchResult {
            title: "Rust async".to_string(),
            url: "https://example.com/rust".to_string(),
            snippet: "Async Rust result.".to_string(),
        }]
    );
}

#[test]
fn handler_uses_web_search_spec_for_allowed_domains() {
    let spec = ToolSpec::WebSearch {
        external_web_access: Some(true),
        index_gated_web_access: None,
        filters: Some(ResponsesApiWebSearchFilters {
            allowed_domains: Some(vec!["*.Example.com".to_string(), "bad/domain".to_string()]),
        }),
        user_location: None,
        search_context_size: Some(WebSearchContextSize::Low),
        search_content_types: None,
    };
    let endpoint = Url::parse("https://example.test/html/").expect("endpoint");
    let handler = WebSearchHandler::new_for_test(spec.clone(), endpoint, reqwest::Client::new());

    assert_eq!(handler.spec(), spec);
    assert_eq!(handler.allowed_domains, vec!["example.com"]);
    assert_eq!(handler.max_results_per_query, 3);
    assert_eq!(handler.unsupported_reason, None);
}

#[tokio::test]
async fn handler_rejects_unsupported_local_semantics_before_network_search() {
    let server = MockServer::start().await;
    let endpoint = Url::parse(&format!("{}/html/", server.uri())).expect("mock endpoint");
    let spec = ToolSpec::WebSearch {
        external_web_access: Some(false),
        index_gated_web_access: None,
        filters: None,
        user_location: None,
        search_context_size: None,
        search_content_types: None,
    };
    let handler = WebSearchHandler::new_for_test(spec, endpoint, reqwest::Client::new());

    let Err(FunctionCallError::RespondToModel(message)) = handler
        .handle(invocation_for_arguments(r#"{"query":"rust async"}"#).await)
        .await
    else {
        panic!("cached-only local fallback should return a model-correctable error")
    };

    assert!(
        message.contains("cached-only"),
        "unexpected error message: {message}"
    );
    assert_eq!(
        server
            .received_requests()
            .await
            .expect("mock server should capture requests")
            .len(),
        0
    );
}

#[test]
fn local_web_search_rejects_non_text_content_types() {
    let spec = ToolSpec::WebSearch {
        external_web_access: Some(true),
        index_gated_web_access: None,
        filters: None,
        user_location: None,
        search_context_size: None,
        search_content_types: Some(vec!["text".to_string(), "image".to_string()]),
    };

    assert_eq!(
        unsupported_local_web_search_reason(&spec),
        Some("web_search local fallback only supports text search results")
    );
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
