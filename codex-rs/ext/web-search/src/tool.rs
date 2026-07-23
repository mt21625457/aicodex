use codex_api::MoonshotSearchClient;
use codex_api::ReqwestTransport;
use codex_api::SearchClient;
use codex_api::SearchCommands;
use codex_api::SearchQuery;
use codex_api::SearchRequest;
use codex_api::SearchSettings;
use codex_config::config_toml::MoonshotSearchConfig;
use codex_core::X_CODEX_TURN_METADATA_HEADER;
use codex_core::web_search_action_detail;
use codex_extension_api::ExtensionTurnItem;
use codex_extension_api::FunctionCallError;
use codex_extension_api::ResponsesApiTool;
use codex_extension_api::ToolCall;
use codex_extension_api::ToolExecutor;
use codex_extension_api::ToolName;
use codex_extension_api::ToolOutput;
use codex_extension_api::ToolSpec;
use codex_extension_api::parse_tool_input_schema_without_compaction;
use codex_extension_items::ExtensionItem;
use codex_extension_items::web_search::WebSearchAction;
use codex_extension_items::web_search::WebSearchItem;
use codex_login::default_client::add_originator_header;
use codex_login::default_client::create_client;
use codex_model_provider::SharedModelProvider;
use codex_protocol::models::WebSearchAction as CoreWebSearchAction;
use codex_protocol::protocol::EventMsg;
use codex_protocol::protocol::WebSearchBeginEvent;
use codex_protocol::protocol::WebSearchEndEvent;
use codex_tools::ResponsesApiNamespace;
use codex_tools::ResponsesApiNamespaceTool;
use codex_tools::ToolExposure;
use codex_tools::default_namespace_description;
use codex_web_search::WebSearchBackendKind;
use codex_web_search::execute_moonshot_search;
use codex_web_search::normalize_moonshot_commands;
use codex_web_search::resolve_moonshot_search_config;
use codex_web_search::select_web_search_backend;
use http::HeaderMap;
use http::HeaderValue;
use url::Url;

use crate::history::recent_input;
use crate::output::SearchOutput;
use crate::schema::commands_schema;

pub(crate) const WEB_NAMESPACE: &str = "web";
pub(crate) const RUN_TOOL_NAME: &str = "run";
const WEB_RUN_DESCRIPTION: &str = include_str!("../web_run_description.md");
const OPENAI_SEARCH_FALLBACK_MODEL: &str = "gpt-5.2-codex";
const RESULTS_PAYLOAD_BYTES_METRIC: &str = "codex.web_search.results.payload_bytes";

pub(crate) struct WebSearchTool {
    pub(crate) session_id: String,
    pub(crate) primary_provider: SharedModelProvider,
    pub(crate) openai_provider: SharedModelProvider,
    pub(crate) moonshot_search: MoonshotSearchConfig,
    pub(crate) moonshot_feature_enabled: bool,
    pub(crate) settings: SearchSettings,
    pub(crate) originator: Option<String>,
}

impl ToolExecutor<ToolCall> for WebSearchTool {
    fn tool_name(&self) -> ToolName {
        ToolName::namespaced(WEB_NAMESPACE, RUN_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        // parse schema without compaction that removes field metadata/descriptions to match hosted tool definition
        let parameters = match parse_tool_input_schema_without_compaction(&commands_schema()) {
            Ok(parameters) => parameters,
            Err(err) => panic!("search command schema should parse: {err}"),
        };

        ToolSpec::Namespace(ResponsesApiNamespace {
            name: WEB_NAMESPACE.to_string(),
            description: default_namespace_description(WEB_NAMESPACE),
            tools: vec![ResponsesApiNamespaceTool::Function(ResponsesApiTool {
                name: RUN_TOOL_NAME.to_string(),
                description: WEB_RUN_DESCRIPTION.to_string(),
                strict: false,
                parameters,
                output_schema: None,
                defer_loading: None,
            })],
        })
    }

    fn exposure(&self) -> ToolExposure {
        ToolExposure::Direct
    }

    fn supports_parallel_tool_calls(&self) -> bool {
        true
    }

    fn handle(&self, call: ToolCall) -> codex_extension_api::ToolExecutorFuture<'_> {
        Box::pin(self.handle_call(call))
    }
}

impl WebSearchTool {
    async fn handle_call(&self, call: ToolCall) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        if select_web_search_backend(
            self.moonshot_feature_enabled,
            self.moonshot_search.enabled,
            &call.model,
        ) == WebSearchBackendKind::MoonshotSimpleSearch
        {
            return self.handle_moonshot_call(call).await;
        }

        let commands = parse_commands(&call)?;
        let command_action = command_action(&commands);
        let provider = self
            .openai_provider
            .api_provider()
            .await
            .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
        let auth = self
            .openai_provider
            .api_auth()
            .await
            .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
        let client = SearchClient::new(
            ReqwestTransport::from_http_client(create_client()),
            provider,
            auth,
        );
        let request = SearchRequest {
            id: self.session_id.clone(),
            model: if self.primary_provider.info().is_openai() {
                call.model.clone()
            } else {
                OPENAI_SEARCH_FALLBACK_MODEL.to_string()
            },
            reasoning: None,
            input: recent_input(call.conversation_history.items()),
            commands: Some(commands),
            settings: Some(self.settings.clone()),
            max_output_tokens: Some(
                u64::try_from(call.truncation_policy.token_budget()).unwrap_or(u64::MAX),
            ),
        };
        let extra_headers = search_request_headers(
            self.originator.as_deref(),
            call.codex_turn_metadata.as_deref(),
        );
        call.turn_item_emitter
            .emit_started(extension_turn_item(
                WebSearchItem {
                    id: call.call_id.clone(),
                    query: String::new(),
                    action: None,
                    results: None,
                },
                EventMsg::WebSearchBegin(WebSearchBeginEvent {
                    call_id: call.call_id.clone(),
                }),
            ))
            .await;
        let response = client
            .search(&request, extra_headers)
            .await
            .map_err(|err| FunctionCallError::Fatal(err.to_string()))?;
        let output = response.output;
        let results = response.results;
        if let Some(results) = results.as_ref()
            && let Some(metrics) = codex_otel::global()
            && let Ok(payload) = serde_json::to_vec(results)
        {
            let payload_bytes = i64::try_from(payload.len()).unwrap_or(i64::MAX);
            let _ = metrics.histogram(RESULTS_PAYLOAD_BYTES_METRIC, payload_bytes, &[]);
        }
        let legacy_action = match &command_action {
            WebSearchAction::Search { query, queries } => CoreWebSearchAction::Search {
                query: query.clone(),
                queries: queries.clone(),
            },
            WebSearchAction::OpenPage { url } => CoreWebSearchAction::OpenPage { url: url.clone() },
            WebSearchAction::FindInPage { url, pattern } => CoreWebSearchAction::FindInPage {
                url: url.clone(),
                pattern: pattern.clone(),
            },
            WebSearchAction::Other => CoreWebSearchAction::Other,
        };
        let query = web_search_action_detail(&legacy_action);
        call.turn_item_emitter
            .emit_completed(extension_turn_item(
                WebSearchItem {
                    id: call.call_id.clone(),
                    query: query.clone(),
                    action: Some(command_action),
                    results: results.clone(),
                },
                EventMsg::WebSearchEnd(WebSearchEndEvent {
                    call_id: call.call_id.clone(),
                    query,
                    action: legacy_action,
                    results,
                }),
            ))
            .await;

        Ok(Box::new(SearchOutput::new(output)))
    }

    async fn handle_moonshot_call(
        &self,
        call: ToolCall,
    ) -> Result<Box<dyn ToolOutput>, FunctionCallError> {
        let arguments = call.function_arguments()?;
        let normalized = normalize_moonshot_commands(arguments);
        let command_action = normalized
            .as_ref()
            .map(|commands| query_action_from_strings(&commands.queries))
            .unwrap_or(WebSearchAction::Other);
        let legacy_action = legacy_action(&command_action);
        let query = web_search_action_detail(&legacy_action);
        call.turn_item_emitter
            .emit_started(extension_turn_item(
                WebSearchItem {
                    id: call.call_id.clone(),
                    query: query.clone(),
                    action: Some(command_action.clone()),
                    results: None,
                },
                EventMsg::WebSearchBegin(WebSearchBeginEvent {
                    call_id: call.call_id.clone(),
                }),
            ))
            .await;

        let execution = match normalized {
            Ok(commands) => self.execute_moonshot(&call, &commands).await,
            Err(error) => Err(FunctionCallError::RespondToModel(error.to_string())),
        };
        let results = execution.as_ref().ok().map(|execution| {
            execution
                .results
                .iter()
                .filter_map(|result| serde_json::to_value(result).ok())
                .collect::<Vec<_>>()
        });
        call.turn_item_emitter
            .emit_completed(extension_turn_item(
                WebSearchItem {
                    id: call.call_id.clone(),
                    query: query.clone(),
                    action: Some(command_action),
                    results: results.clone(),
                },
                EventMsg::WebSearchEnd(WebSearchEndEvent {
                    call_id: call.call_id.clone(),
                    query,
                    action: legacy_action,
                    results,
                }),
            ))
            .await;

        let execution = execution?;
        Ok(Box::new(SearchOutput::new(execution.output)))
    }

    async fn execute_moonshot(
        &self,
        call: &ToolCall,
        commands: &codex_web_search::NormalizedMoonshotCommands,
    ) -> Result<codex_web_search::MoonshotSearchExecution, FunctionCallError> {
        let provider = self.primary_provider.api_provider().await.map_err(|_| {
            FunctionCallError::RespondToModel(
                "Moonshot web search could not resolve the primary provider URL".to_string(),
            )
        })?;
        let provider_bearer = self
            .primary_provider
            .api_auth()
            .await
            .ok()
            .and_then(|auth| {
                codex_web_search::provider_token_from_auth_headers(&auth.to_auth_headers())
            });
        let resolved = resolve_moonshot_search_config(
            &self.moonshot_search,
            Some(&provider.base_url),
            provider_bearer.as_deref(),
        )
        .map_err(|error| FunctionCallError::RespondToModel(error.to_string()))?;
        let client = MoonshotSearchClient::new(
            codex_api::build_moonshot_search_http_client()
                .map_err(|error| FunctionCallError::RespondToModel(error.to_string()))?,
            resolved.url,
            resolved.bearer_token,
            resolved.custom_headers,
        )
        .map_err(|error| FunctionCallError::RespondToModel(error.to_string()))?;
        execute_moonshot_search(
            &client,
            commands,
            Some(&call.call_id),
            call.truncation_policy.token_budget(),
        )
        .await
        .map_err(|error| FunctionCallError::RespondToModel(error.to_string()))
    }
}

fn legacy_action(action: &WebSearchAction) -> CoreWebSearchAction {
    match action {
        WebSearchAction::Search { query, queries } => CoreWebSearchAction::Search {
            query: query.clone(),
            queries: queries.clone(),
        },
        WebSearchAction::OpenPage { url } => CoreWebSearchAction::OpenPage { url: url.clone() },
        WebSearchAction::FindInPage { url, pattern } => CoreWebSearchAction::FindInPage {
            url: url.clone(),
            pattern: pattern.clone(),
        },
        WebSearchAction::Other => CoreWebSearchAction::Other,
    }
}

fn query_action_from_strings(queries: &[String]) -> WebSearchAction {
    match queries {
        [] => WebSearchAction::Other,
        [query] => WebSearchAction::Search {
            query: Some(query.clone()),
            queries: None,
        },
        queries => WebSearchAction::Search {
            query: None,
            queries: Some(queries.to_vec()),
        },
    }
}

fn search_request_headers(originator: Option<&str>, turn_metadata: Option<&str>) -> HeaderMap {
    let mut headers = HeaderMap::new();
    if let Some(turn_metadata) = turn_metadata
        && let Ok(header_value) = HeaderValue::from_str(turn_metadata)
    {
        headers.insert(X_CODEX_TURN_METADATA_HEADER, header_value);
    }

    if let Some(originator) = originator {
        add_originator_header(&mut headers, originator);
    }
    headers
}

fn parse_commands(call: &ToolCall) -> Result<SearchCommands, FunctionCallError> {
    let arguments = call.function_arguments()?;
    if arguments.trim().is_empty() {
        return Ok(SearchCommands::default());
    }

    serde_json::from_str(arguments)
        .map_err(|err| FunctionCallError::RespondToModel(err.to_string()))
}

fn command_action(commands: &SearchCommands) -> WebSearchAction {
    commands
        .search_query
        .as_deref()
        .and_then(query_action)
        .or_else(|| commands.image_query.as_deref().and_then(query_action))
        .or_else(|| {
            commands
                .open
                .as_deref()
                .and_then(|operations| operations.first())
                .and_then(|operation| {
                    literal_url(&operation.ref_id)
                        .map(|url| WebSearchAction::OpenPage { url: Some(url) })
                })
        })
        .or_else(|| {
            commands
                .find
                .as_deref()
                .and_then(|operations| operations.first())
                .map(|operation| WebSearchAction::FindInPage {
                    url: literal_url(&operation.ref_id),
                    pattern: Some(operation.pattern.clone()),
                })
        })
        .unwrap_or(WebSearchAction::Other)
}

fn query_action(queries: &[SearchQuery]) -> Option<WebSearchAction> {
    match queries {
        [] => None,
        [query] => Some(WebSearchAction::Search {
            query: Some(query.q.clone()),
            queries: None,
        }),
        queries => Some(WebSearchAction::Search {
            query: None,
            queries: Some(queries.iter().map(|query| query.q.clone()).collect()),
        }),
    }
}

fn literal_url(ref_id: &str) -> Option<String> {
    Url::parse(ref_id).is_ok().then(|| ref_id.to_string())
}

fn extension_turn_item(item: WebSearchItem, legacy_event: EventMsg) -> ExtensionTurnItem {
    ExtensionTurnItem {
        item: ExtensionItem::WebSearch(item),
        legacy_events: vec![legacy_event],
    }
}

#[cfg(test)]
mod tests {
    use codex_api::SearchCommands;
    use codex_config::config_toml::MoonshotSearchConfig;
    use codex_extension_api::ConversationHistory;
    use codex_extension_api::ExtensionTurnItem;
    use codex_extension_api::ToolCall;
    use codex_extension_api::ToolPayload;
    use codex_extension_api::TurnItemEmissionFuture;
    use codex_extension_api::TurnItemEmitter;
    use codex_extension_items::ExtensionItem;
    use codex_extension_items::web_search::WebSearchAction;
    use codex_model_provider::create_model_provider;
    use codex_model_provider_info::ModelProviderInfo;
    use codex_model_provider_info::WireApi;
    use codex_model_provider_info::create_oss_provider_with_base_url;
    use codex_protocol::protocol::EventMsg;
    use codex_utils_output_truncation::TruncationPolicy;
    use pretty_assertions::assert_eq;
    use serde_json::json;
    use std::sync::Arc;
    use std::sync::Mutex;
    use wiremock::Mock;
    use wiremock::MockServer;
    use wiremock::ResponseTemplate;
    use wiremock::matchers::method;
    use wiremock::matchers::path;

    use super::OPENAI_SEARCH_FALLBACK_MODEL;
    use super::WebSearchTool;
    use super::command_action;
    use super::search_request_headers;
    use codex_core::X_CODEX_TURN_METADATA_HEADER;

    #[test]
    fn search_request_headers_forward_thread_originator_and_turn_metadata() {
        let headers = search_request_headers(Some("chatgpt_cca"), Some("turn-metadata"));
        assert_eq!(
            headers
                .get("originator")
                .and_then(|value| value.to_str().ok()),
            Some("chatgpt_cca")
        );
        assert_eq!(
            headers
                .get(X_CODEX_TURN_METADATA_HEADER)
                .and_then(|value| value.to_str().ok()),
            Some("turn-metadata")
        );
    }

    #[derive(Default)]
    struct RecordingEmitter {
        started: Mutex<Vec<ExtensionTurnItem>>,
        completed: Mutex<Vec<ExtensionTurnItem>>,
    }

    impl TurnItemEmitter for RecordingEmitter {
        fn emit_started<'a>(&'a self, item: ExtensionTurnItem) -> TurnItemEmissionFuture<'a> {
            self.started.lock().expect("started lock").push(item);
            Box::pin(std::future::ready(()))
        }

        fn emit_completed<'a>(&'a self, item: ExtensionTurnItem) -> TurnItemEmissionFuture<'a> {
            self.completed.lock().expect("completed lock").push(item);
            Box::pin(std::future::ready(()))
        }
    }

    fn kimi_tool(server: &MockServer) -> WebSearchTool {
        let mut primary =
            create_oss_provider_with_base_url(&format!("{}/v1", server.uri()), WireApi::Claude);
        primary.experimental_bearer_token = Some("provider-token".to_string());
        let mut openai =
            ModelProviderInfo::create_openai_provider(Some(format!("{}/v1", server.uri())));
        openai.requires_openai_auth = false;
        openai.http_headers = None;
        WebSearchTool {
            session_id: "session-1".to_string(),
            primary_provider: create_model_provider(primary, /*auth_manager*/ None),
            openai_provider: create_model_provider(openai, /*auth_manager*/ None),
            moonshot_search: MoonshotSearchConfig {
                base_url: Some(format!("{}/v1/search", server.uri())),
                api_key: Some("moonshot-token".to_string()),
                ..Default::default()
            },
            moonshot_feature_enabled: true,
            settings: Default::default(),
            originator: None,
        }
    }

    fn kimi_call(arguments: serde_json::Value, emitter: Arc<dyn TurnItemEmitter>) -> ToolCall {
        ToolCall {
            turn_id: "turn-1".to_string(),
            call_id: "call-1".to_string(),
            tool_name: codex_extension_api::ToolName::namespaced("web", "run"),
            model: "gateway:k3".to_string(),
            codex_turn_metadata: None,
            truncation_policy: TruncationPolicy::Bytes(32_000),
            conversation_history: ConversationHistory::default(),
            turn_item_emitter: emitter,
            environments: Vec::new(),
            payload: ToolPayload::Function {
                arguments: arguments.to_string(),
            },
        }
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
            let commands: SearchCommands =
                serde_json::from_str(arguments).expect("valid search command arguments");
            assert_eq!(command_action(&commands), expected);
        }
    }

    #[tokio::test]
    async fn kimi_web_run_uses_moonshot_and_emits_structured_completion() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "search_results": [{
                    "title": "Rust release",
                    "url": "https://example.com/rust",
                    "snippet": "Rust is current.",
                    "content": "full page must be dropped"
                }]
            })))
            .expect(1)
            .mount(&server)
            .await;
        let emitter = Arc::new(RecordingEmitter::default());
        let output = kimi_tool(&server)
            .handle_call(kimi_call(json!({"query": "rust release"}), emitter.clone()))
            .await
            .expect("Moonshot web.run should succeed");

        assert!(output.contains_external_context());
        let started = emitter.started.lock().expect("started lock");
        let completed = emitter.completed.lock().expect("completed lock");
        assert_eq!(started.len(), 1);
        assert_eq!(completed.len(), 1);
        assert!(matches!(
            started[0].legacy_events.as_slice(),
            [EventMsg::WebSearchBegin(_)]
        ));
        assert!(matches!(
            completed[0].legacy_events.as_slice(),
            [EventMsg::WebSearchEnd(_)]
        ));
        let ExtensionItem::WebSearch(item) = &completed[0].item else {
            panic!("expected web-search completion item");
        };
        assert_eq!(
            item.results,
            Some(vec![json!({
                "type": "text_result",
                "title": "Rust release",
                "url": "https://example.com/rust",
                "snippet": "Rust is current."
            })])
        );
    }

    #[tokio::test]
    async fn rejected_kimi_web_run_still_closes_the_event_lifecycle() {
        let server = MockServer::start().await;
        let emitter = Arc::new(RecordingEmitter::default());
        let result = kimi_tool(&server)
            .handle_call(kimi_call(
                json!({"query": "rust", "open": [{"ref_id": "x"}]}),
                emitter.clone(),
            ))
            .await;
        let Err(error) = result else {
            panic!("rich Moonshot command should be rejected");
        };

        assert!(matches!(
            error,
            codex_extension_api::FunctionCallError::RespondToModel(_)
        ));
        assert_eq!(emitter.started.lock().expect("started lock").len(), 1);
        assert_eq!(emitter.completed.lock().expect("completed lock").len(), 1);
        assert_eq!(
            server
                .received_requests()
                .await
                .expect("request capture")
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn standalone_kill_switches_and_non_kimi_use_openai_fallback() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/alpha/search"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output": "OpenAI fallback result",
                "results": []
            })))
            .expect(3)
            .mount(&server)
            .await;

        let emitter = Arc::new(RecordingEmitter::default());
        let mut feature_off = kimi_tool(&server);
        feature_off.moonshot_feature_enabled = false;
        feature_off
            .handle_call(kimi_call(json!({"query": "feature off"}), emitter.clone()))
            .await
            .expect("feature kill switch should use OpenAI");

        let mut config_off = kimi_tool(&server);
        config_off.moonshot_search.enabled = false;
        config_off
            .handle_call(kimi_call(json!({"query": "config off"}), emitter.clone()))
            .await
            .expect("config kill switch should use OpenAI");

        let mut non_kimi_call = kimi_call(json!({"query": "non kimi"}), emitter);
        non_kimi_call.model = "claude-sonnet-4-5".to_string();
        kimi_tool(&server)
            .handle_call(non_kimi_call)
            .await
            .expect("non-Kimi model should use OpenAI");

        let requests = server
            .received_requests()
            .await
            .expect("requests should be captured");
        assert_eq!(requests.len(), 3);
        assert!(
            requests
                .iter()
                .all(|request| request.url.path() == "/v1/alpha/search")
        );
        for request in requests {
            let body: serde_json::Value =
                serde_json::from_slice(&request.body).expect("OpenAI search request JSON");
            assert_eq!(body["model"], OPENAI_SEARCH_FALLBACK_MODEL);
        }
    }

    #[tokio::test]
    async fn cross_origin_moonshot_without_independent_token_sends_no_request() {
        let server = MockServer::start().await;
        let emitter = Arc::new(RecordingEmitter::default());
        let mut tool = kimi_tool(&server);
        tool.moonshot_search.api_key = None;
        let mut primary =
            create_oss_provider_with_base_url("https://provider.example/v1", WireApi::Claude);
        primary.experimental_bearer_token = Some("provider-token".to_string());
        tool.primary_provider = create_model_provider(primary, /*auth_manager*/ None);
        let result = tool
            .handle_call(kimi_call(
                json!({"query": "credential isolation"}),
                emitter.clone(),
            ))
            .await;
        let Err(error) = result else {
            panic!("cross-origin provider token reuse should fail");
        };
        let message = error.to_string();

        assert!(message.contains("independent credential"));
        assert!(!message.contains("provider-token"));
        assert_eq!(emitter.started.lock().expect("started lock").len(), 1);
        assert_eq!(emitter.completed.lock().expect("completed lock").len(), 1);
        assert_eq!(
            server
                .received_requests()
                .await
                .expect("request capture")
                .len(),
            0
        );
    }
}
