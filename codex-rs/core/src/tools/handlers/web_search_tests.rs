use super::*;
use codex_protocol::config_types::WebSearchContextSize;
use codex_tools::ResponsesApiWebSearchFilters;
use pretty_assertions::assert_eq;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;
use wiremock::matchers::path;
use wiremock::matchers::query_param;

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
fn handler_keeps_web_search_spec_and_allowed_domains() {
    let spec = ToolSpec::WebSearch {
        external_web_access: Some(true),
        filters: Some(ResponsesApiWebSearchFilters {
            allowed_domains: Some(vec!["*.Example.com".to_string(), "bad/domain".to_string()]),
        }),
        user_location: None,
        search_context_size: Some(WebSearchContextSize::Low),
        search_content_types: None,
    };
    let endpoint = Url::parse("https://example.test/html/").expect("endpoint");
    let handler = WebSearchHandler::new_for_test(spec.clone(), endpoint, reqwest::Client::new());

    assert_eq!(handler.spec().as_ref(), Some(&spec));
    assert_eq!(handler.allowed_domains, vec!["example.com"]);
}
