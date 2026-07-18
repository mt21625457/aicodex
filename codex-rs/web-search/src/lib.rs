mod commands;
mod config;
mod output;

pub use commands::MoonshotCommandError;
pub use commands::NormalizedMoonshotCommands;
pub use commands::normalize_moonshot_commands;
pub use config::MoonshotConfigError;
pub use config::ResolvedMoonshotSearchConfig;
pub use config::bearer_token_from_headers;
pub use config::provider_token_from_auth_headers;
pub use config::resolve_moonshot_search_config;
pub use config::validate_moonshot_search_config;
pub use output::BoundedSearchResult;
pub use output::MoonshotSearchExecution;
pub use output::execute_moonshot_search;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WebSearchBackendKind {
    OpenAiAlphaSearch,
    MoonshotSimpleSearch,
}

pub fn select_web_search_backend(
    feature_enabled: bool,
    moonshot_enabled: bool,
    model_slug: &str,
) -> WebSearchBackendKind {
    if feature_enabled
        && moonshot_enabled
        && codex_model_provider_info::is_kimi_model_slug(model_slug)
    {
        WebSearchBackendKind::MoonshotSimpleSearch
    } else {
        WebSearchBackendKind::OpenAiAlphaSearch
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;

    #[test]
    fn backend_selection_requires_both_switches_and_a_kimi_slug() {
        assert_eq!(
            select_web_search_backend(true, true, "gateway:k3"),
            WebSearchBackendKind::MoonshotSimpleSearch
        );
        for selected in [
            select_web_search_backend(false, true, "k3"),
            select_web_search_backend(true, false, "k3"),
            select_web_search_backend(true, true, "claude-sonnet-4-5"),
        ] {
            assert_eq!(selected, WebSearchBackendKind::OpenAiAlphaSearch);
        }
    }
}
