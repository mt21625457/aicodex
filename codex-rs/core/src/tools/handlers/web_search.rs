use crate::function_tool::FunctionCallError;
use crate::tools::context::ToolInvocation;
use crate::tools::context::ToolOutput;
use crate::tools::context::ToolPayload;
use crate::tools::context::boxed_tool_output;
use crate::tools::registry::CoreToolRuntime;
use crate::tools::registry::ToolExecutor;
use crate::web_search::web_search_action_detail;
use codex_api::AllowedCaller;
use codex_api::ApproximateLocation;
use codex_api::ExternalWebAccess;
use codex_api::ExternalWebAccessMode;
use codex_api::LocationType;
use codex_api::ReqwestTransport;
use codex_api::SearchClient;
use codex_api::SearchCommands;
use codex_api::SearchContextSize;
use codex_api::SearchFilters;
use codex_api::SearchInput;
use codex_api::SearchQuery;
use codex_api::SearchRequest;
use codex_api::SearchResponse;
use codex_api::SearchSettings;
use codex_login::default_client::build_reqwest_client;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::items::TurnItem as ProtocolTurnItem;
use codex_protocol::items::WebSearchItem;
use codex_protocol::models::ContentItem;
use codex_protocol::models::FunctionCallOutputContentItem;
use codex_protocol::models::FunctionCallOutputPayload;
use codex_protocol::models::ResponseInputItem;
use codex_protocol::models::ResponseItem;
use codex_protocol::models::WebSearchAction;
use codex_protocol::models::plaintext_agent_message_content;
use codex_tools::ToolName;
use codex_tools::ToolSpec;
use codex_tools::retain_tail_from_last_n_user_messages;
use codex_tools::truncate_assistant_output_text_to_token_budget;
use serde::Deserialize;
use serde_json::Value;
use url::Url;

const WEB_SEARCH_TOOL_NAME: &str = "web_search";
const OPENAI_SEARCH_FALLBACK_MODEL: &str = "gpt-5.2-codex";
const ASSISTANT_CONTEXT_TOKEN_LIMIT: usize = 1_000;
const ASSISTANT_ROLE: &str = "assistant";
const USER_ROLE: &str = "user";

pub struct WebSearchHandler {
    spec: ToolSpec,
    settings: SearchSettings,
    #[cfg(test)]
    search_provider_override: Option<ModelProviderInfo>,
    #[cfg(test)]
    search_model_override: Option<String>,
}

impl WebSearchHandler {
    pub(crate) fn new(spec: ToolSpec) -> Self {
        let settings = search_settings_from_spec(&spec);
        Self {
            spec,
            settings,
            #[cfg(test)]
            search_provider_override: None,
            #[cfg(test)]
            search_model_override: None,
        }
    }

    #[cfg(test)]
    fn new_for_test(
        spec: ToolSpec,
        search_provider_override: ModelProviderInfo,
        search_model_override: impl Into<String>,
    ) -> Self {
        let settings = search_settings_from_spec(&spec);
        Self {
            spec,
            settings,
            search_provider_override: Some(search_provider_override),
            search_model_override: Some(search_model_override.into()),
        }
    }

    async fn run_openai_web_search(
        &self,
        session: &crate::session::session::Session,
        turn: &crate::session::turn_context::TurnContext,
        commands: SearchCommands,
    ) -> Result<SearchResponse, FunctionCallError> {
        let provider_info = self.search_provider_info(turn);
        let provider = create_model_provider(provider_info, turn.auth_manager.clone());
        let api_provider = provider.api_provider().await.map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "web_search failed to initialize OpenAI search provider: {err}"
            ))
        })?;
        let auth = provider.api_auth().await.map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "web_search failed to initialize OpenAI search auth: {err}"
            ))
        })?;
        let client = SearchClient::new(
            ReqwestTransport::new(build_reqwest_client()),
            api_provider,
            auth,
        );
        let history = session.clone_history().await;
        let request = SearchRequest {
            id: session.session_id().to_string(),
            model: self.search_model(turn),
            reasoning: None,
            input: recent_input(history.raw_items()),
            commands: Some(commands),
            settings: Some(self.settings.clone()),
            max_output_tokens: Some(search_output_token_budget(turn)),
        };
        client
            .search(&request, http::HeaderMap::new())
            .await
            .map_err(|err| FunctionCallError::RespondToModel(format!("web_search failed: {err}")))
    }

    fn search_provider_info(
        &self,
        turn: &crate::session::turn_context::TurnContext,
    ) -> ModelProviderInfo {
        #[cfg(test)]
        if let Some(provider) = &self.search_provider_override {
            return provider.clone();
        }

        turn.config
            .model_providers
            .get(OPENAI_PROVIDER_ID)
            .cloned()
            .unwrap_or_else(|| ModelProviderInfo::create_openai_provider(/*base_url*/ None))
    }

    fn search_model(&self, turn: &crate::session::turn_context::TurnContext) -> String {
        #[cfg(test)]
        if let Some(model) = &self.search_model_override {
            return model.clone();
        }

        if turn.provider.info().is_openai() {
            turn.model_info.slug.clone()
        } else {
            OPENAI_SEARCH_FALLBACK_MODEL.to_string()
        }
    }
}

#[derive(Debug, Deserialize)]
struct WebSearchArgs {
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    queries: Option<Vec<String>>,
}

struct OpenAiSearchOutput {
    output: String,
}

impl ToolExecutor<ToolInvocation> for WebSearchHandler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain(WEB_SEARCH_TOOL_NAME)
    }

    fn spec(&self) -> ToolSpec {
        self.spec.clone()
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move {
            let ToolInvocation {
                session,
                turn,
                payload,
                call_id,
                ..
            } = invocation;
            let arguments = match payload {
                ToolPayload::Function { arguments } => arguments,
                _ => {
                    return Err(FunctionCallError::RespondToModel(
                        "web_search handler received unsupported payload".to_string(),
                    ));
                }
            };

            let commands = parse_search_commands(&arguments)?;
            let action = command_action(&commands);
            let query_detail = web_search_action_detail(&action);
            let item = ProtocolTurnItem::WebSearch(WebSearchItem {
                id: call_id.clone(),
                query: query_detail,
                action,
                results: None,
            });
            session.emit_turn_item_started(turn.as_ref(), &item).await;
            let result = self
                .run_openai_web_search(session.as_ref(), turn.as_ref(), commands)
                .await;
            session.emit_turn_item_completed(turn.as_ref(), item).await;

            let response = result?;
            Ok(boxed_tool_output(OpenAiSearchOutput {
                output: response.output,
            }))
        })
    }
}

impl CoreToolRuntime for WebSearchHandler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

fn search_settings_from_spec(spec: &ToolSpec) -> SearchSettings {
    let ToolSpec::WebSearch {
        external_web_access,
        indexed_web_access,
        filters,
        user_location,
        search_context_size,
        ..
    } = spec
    else {
        return SearchSettings::default();
    };

    SearchSettings {
        user_location: user_location.as_ref().map(|location| ApproximateLocation {
            r#type: LocationType::Approximate,
            country: location.country.clone(),
            region: location.region.clone(),
            city: location.city.clone(),
            timezone: location.timezone.clone(),
        }),
        search_context_size: search_context_size.map(|size| match size {
            WebSearchContextSize::Low => SearchContextSize::Low,
            WebSearchContextSize::Medium => SearchContextSize::Medium,
            WebSearchContextSize::High => SearchContextSize::High,
        }),
        filters: filters.as_ref().map(|filters| SearchFilters {
            allowed_domains: filters.allowed_domains.clone(),
            blocked_domains: None,
        }),
        allowed_callers: Some(vec![AllowedCaller::Direct]),
        external_web_access: external_web_access_for_spec(
            *external_web_access,
            *indexed_web_access,
        ),
        ..Default::default()
    }
}

fn external_web_access_for_spec(
    external_web_access: Option<bool>,
    indexed_web_access: Option<bool>,
) -> Option<ExternalWebAccess> {
    if indexed_web_access == Some(true) {
        return Some(ExternalWebAccess::Mode(ExternalWebAccessMode::Indexed));
    }
    external_web_access.map(ExternalWebAccess::Boolean)
}

fn search_output_token_budget(turn: &crate::session::turn_context::TurnContext) -> u64 {
    let truncation_policy: codex_utils_output_truncation::TruncationPolicy =
        turn.model_info.truncation_policy.into();
    u64::try_from(truncation_policy.token_budget()).unwrap_or(u64::MAX)
}

fn parse_search_commands(arguments: &str) -> Result<SearchCommands, FunctionCallError> {
    if arguments.trim().is_empty() {
        return Ok(SearchCommands::default());
    }

    let value: Value = serde_json::from_str(arguments).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to parse function arguments: {err}"))
    })?;
    let mut commands: SearchCommands = serde_json::from_value(value.clone()).map_err(|err| {
        FunctionCallError::RespondToModel(format!("failed to parse web_search commands: {err}"))
    })?;
    let alias_queries = legacy_query_aliases(&value)?;
    if !alias_queries.is_empty() {
        let search_query = commands.search_query.get_or_insert_with(Vec::new);
        for query in alias_queries.into_iter().rev() {
            search_query.insert(0, query);
        }
    }
    Ok(commands)
}

fn legacy_query_aliases(value: &Value) -> Result<Vec<SearchQuery>, FunctionCallError> {
    let args: WebSearchArgs = serde_json::from_value(value.clone()).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to parse web_search query aliases: {err}"
        ))
    })?;
    let mut queries = Vec::new();
    if let Some(query) = args.query {
        push_search_query(&mut queries, query);
    }
    if let Some(batch) = args.queries {
        for query in batch {
            push_search_query(&mut queries, query);
        }
    }
    Ok(queries)
}

fn push_search_query(queries: &mut Vec<SearchQuery>, query: String) {
    let query = query.trim();
    if !query.is_empty() && !queries.iter().any(|existing| existing.q == query) {
        queries.push(SearchQuery {
            q: query.to_string(),
            recency: None,
            domains: None,
        });
    }
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

fn recent_input(items: &[ResponseItem]) -> Option<SearchInput> {
    let mut messages = Vec::new();
    for item in items {
        push_visible_message(&mut messages, item);
    }

    retain_tail_from_last_n_user_messages(&mut messages, /*user_message_count*/ 2);
    truncate_assistant_output_text_to_token_budget(&mut messages, ASSISTANT_CONTEXT_TOKEN_LIMIT);
    (!messages.is_empty()).then_some(SearchInput::Items(messages))
}

fn push_visible_message(messages: &mut Vec<ResponseItem>, item: &ResponseItem) {
    match item {
        ResponseItem::Message { role, .. } if role == ASSISTANT_ROLE => {
            let mut message = item.clone();
            message.set_id(/*new_id*/ None);
            messages.push(message);
        }
        ResponseItem::AgentMessage {
            author,
            content,
            internal_chat_message_metadata_passthrough: metadata,
            ..
        } => {
            if let Some(text) = plaintext_agent_message_content(content) {
                messages.push(ResponseItem::Message {
                    id: None,
                    role: ASSISTANT_ROLE.to_string(),
                    content: vec![ContentItem::OutputText {
                        text: format!("Agent message from {author}:\n{text}"),
                    }],
                    phase: None,
                    internal_chat_message_metadata_passthrough: metadata.clone(),
                });
            }
        }
        ResponseItem::Message {
            id: _,
            role,
            content,
            phase,
            internal_chat_message_metadata_passthrough: metadata,
        } if role == USER_ROLE
            && matches!(
                crate::parse_turn_item(item),
                Some(ProtocolTurnItem::UserMessage(_))
            ) =>
        {
            let content = content
                .iter()
                .filter(|item| matches!(item, ContentItem::InputText { .. }))
                .cloned()
                .collect::<Vec<_>>();
            if !content.is_empty() {
                messages.push(ResponseItem::Message {
                    id: None,
                    role: role.clone(),
                    content,
                    phase: phase.clone(),
                    internal_chat_message_metadata_passthrough: metadata.clone(),
                });
            }
        }
        _ => {}
    }
}

impl ToolOutput for OpenAiSearchOutput {
    fn log_preview(&self) -> String {
        "[openai web search output]".to_string()
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn contains_external_context(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, _payload: &ToolPayload) -> ResponseInputItem {
        ResponseInputItem::FunctionCallOutput {
            call_id: call_id.to_string(),
            output: FunctionCallOutputPayload::from_content_items(vec![
                FunctionCallOutputContentItem::InputText {
                    text: self.output.clone(),
                },
            ]),
        }
    }
}

#[cfg(test)]
#[path = "web_search_tests.rs"]
mod tests;
