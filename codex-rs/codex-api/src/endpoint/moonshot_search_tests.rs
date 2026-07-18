use super::*;
use http::Method;
use pretty_assertions::assert_eq;
use serde_json::json;
use wiremock::Mock;
use wiremock::MockServer;
use wiremock::ResponseTemplate;
use wiremock::matchers::method;

fn no_redirect_client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .expect("no-redirect client")
}

fn client(server: &MockServer, path: &str) -> MoonshotSearchClient {
    MoonshotSearchClient::new(
        no_redirect_client(),
        Url::parse(&format!("{}{path}", server.uri())).expect("valid mock URL"),
        "moonshot-token".to_string(),
        HeaderMap::new(),
    )
    .expect("valid Moonshot client")
}

#[tokio::test]
async fn posts_exact_request_and_conditionally_forwards_call_id() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "search_results": []
        })))
        .mount(&server)
        .await;
    let client = client(&server, "/v1/search?source=test");

    client
        .search("rust async", Some("call-1"))
        .await
        .expect("request with call id should succeed");
    client
        .search("rust sync", None)
        .await
        .expect("request without call id should succeed");

    let requests = server
        .received_requests()
        .await
        .expect("requests should be captured");
    assert_eq!(requests.len(), 2);
    assert_eq!(requests[0].method, Method::POST.as_str());
    assert_eq!(requests[0].url.path(), "/v1/search");
    assert_eq!(requests[0].url.query(), Some("source=test"));
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&requests[0].body).expect("JSON request body"),
        json!({"text_query": "rust async"})
    );
    assert_eq!(
        requests[0]
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok()),
        Some("Bearer moonshot-token")
    );
    assert_eq!(
        requests[0]
            .headers
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok()),
        Some("application/json")
    );
    assert_eq!(
        requests[0]
            .headers
            .get(MOONSHOT_TOOL_CALL_ID_HEADER)
            .and_then(|value| value.to_str().ok()),
        Some("call-1")
    );
    assert!(
        requests[1]
            .headers
            .get(MOONSHOT_TOOL_CALL_ID_HEADER)
            .is_none()
    );
}

#[tokio::test]
async fn maps_http_errors_without_unbounded_diagnostics() {
    for (path, status) in [("/unauthorized", 401), ("/failed", 503)] {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(status).set_body_string("bounded failure"))
            .mount(&server)
            .await;
        let error = client(&server, path)
            .search("query", None)
            .await
            .expect_err("non-success response should fail");
        match error {
            MoonshotSearchError::Http {
                status: actual,
                diagnostic,
            } => {
                assert_eq!(actual.as_u16(), status);
                assert_eq!(diagnostic, "bounded failure");
            }
            other => panic!("unexpected error: {other}"),
        }
    }

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(500).set_body_bytes(vec![
            b'x';
            MOONSHOT_SEARCH_MAX_ERROR_BYTES
                + 1
        ]))
        .mount(&server)
        .await;
    let error = client(&server, "/oversized-error")
        .search("query", None)
        .await
        .expect_err("oversized diagnostic should fail closed");
    match error {
        MoonshotSearchError::Http { status, diagnostic } => {
            assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(
                diagnostic,
                "response body exceeded the 16 KiB diagnostic limit"
            );
        }
        other => panic!("unexpected error: {other}"),
    }
}

#[tokio::test]
async fn http_diagnostics_redact_credentials_and_custom_header_values() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(500)
                .set_body_string("echo moonshot-token and private-header-value"),
        )
        .mount(&server)
        .await;
    let mut custom_headers = HeaderMap::new();
    custom_headers.insert(
        "x-private",
        HeaderValue::from_static("private-header-value"),
    );
    let client = MoonshotSearchClient::new(
        no_redirect_client(),
        Url::parse(&format!("{}/search", server.uri())).expect("mock URL"),
        "moonshot-token".to_string(),
        custom_headers,
    )
    .expect("valid client");
    let error = client
        .search("query", None)
        .await
        .expect_err("server error should fail");
    let message = error.to_string();
    let MoonshotSearchError::Http { diagnostic, .. } = error else {
        panic!("expected HTTP error");
    };

    assert!(!message.contains("moonshot-token"));
    assert!(!message.contains("private-header-value"));
    assert!(!message.contains("[REDACTED]"));
    assert_eq!(diagnostic, "echo [REDACTED] and [REDACTED]");
}

#[tokio::test]
async fn rejects_oversized_success_and_drops_content_fields() {
    let oversized_server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![
            b'x';
            MOONSHOT_SEARCH_MAX_RESPONSE_BYTES
                + 1
        ]))
        .mount(&oversized_server)
        .await;
    assert!(matches!(
        client(&oversized_server, "/search")
            .search("query", None)
            .await,
        Err(MoonshotSearchError::ResponseTooLarge)
    ));

    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "search_results": [{
                "title": "Title",
                "url": "https://example.com",
                "snippet": "Snippet",
                "content": "full page must be ignored",
                "unknown": {"nested": true}
            }],
            "unknown_top_level": true
        })))
        .mount(&server)
        .await;
    assert_eq!(
        client(&server, "/search")
            .search("query", None)
            .await
            .expect("unknown fields should be ignored"),
        MoonshotSearchResponse {
            search_results: vec![MoonshotSearchResult {
                title: "Title".to_string(),
                url: "https://example.com".to_string(),
                snippet: "Snippet".to_string(),
                date: None,
                site_name: None,
            }],
            omitted_results: 0,
        }
    );
}

#[tokio::test]
async fn refuses_redirects_without_following() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", "https://evil.example/steal"),
        )
        .mount(&server)
        .await;
    let error = client(&server, "/search")
        .search("query", None)
        .await
        .expect_err("redirects must fail closed");
    assert!(matches!(
        error,
        MoonshotSearchError::Redirect {
            status
        } if status.as_u16() == 302
    ));
}

#[tokio::test]
async fn bounds_search_results_during_decode() {
    let server = MockServer::start().await;
    let results = (0..20)
        .map(|index| {
            json!({
                "title": format!("t{index}"),
                "url": format!("https://example.com/{index}"),
                "snippet": format!("s{index}"),
            })
        })
        .collect::<Vec<_>>();
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "search_results": results
        })))
        .mount(&server)
        .await;
    let response = client(&server, "/search")
        .search("query", None)
        .await
        .expect("bounded decode should succeed");
    assert_eq!(response.search_results.len(), MOONSHOT_SEARCH_MAX_RESULTS);
    assert_eq!(response.omitted_results, 12);
}
