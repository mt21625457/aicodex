use std::sync::Arc;

use codex_api::AllowedCaller;
use codex_api::ApproximateLocation;
use codex_api::ExternalWebAccess;
use codex_api::ExternalWebAccessMode;
use codex_api::LocationType;
use codex_api::SearchContextSize;
use codex_api::SearchFilters;
use codex_api::SearchSettings;
use codex_core::config::Config;
use codex_extension_api::ConfigContributor;
use codex_extension_api::ExtensionData;
use codex_extension_api::ExtensionFuture;
use codex_extension_api::ExtensionRegistryBuilder;
use codex_extension_api::ThreadLifecycleContributor;
use codex_extension_api::ThreadStartInput;
use codex_extension_api::ToolContributor;
use codex_features::Feature;
use codex_login::AuthManager;
use codex_model_provider::create_model_provider;
use codex_model_provider_info::ModelProviderInfo;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_protocol::config_types::WebSearchContextSize;
use codex_protocol::config_types::WebSearchMode;

use crate::tool::WebSearchTool;

#[derive(Clone)]
struct WebSearchExtension {
    auth_manager: Arc<AuthManager>,
}

#[derive(Clone)]
struct WebSearchExtensionConfig {
    available: bool,
    primary_provider: ModelProviderInfo,
    openai_provider: ModelProviderInfo,
    moonshot_search: codex_config::config_toml::MoonshotSearchConfig,
    moonshot_feature_enabled: bool,
    settings: SearchSettings,
}

impl From<&Config> for WebSearchExtensionConfig {
    fn from(config: &Config) -> Self {
        let web_search_mode = config.web_search_mode.value();
        let openai_provider = if config.model_provider.is_openai() {
            config.model_provider.clone()
        } else {
            config
                .model_providers
                .get(OPENAI_PROVIDER_ID)
                .cloned()
                .unwrap_or_else(|| {
                    ModelProviderInfo::create_openai_provider(/*base_url*/ None)
                })
        };
        Self {
            // Core filters this candidate per turn using the actual selected model/provider.
            available: web_search_mode != WebSearchMode::Disabled,
            primary_provider: config.model_provider.clone(),
            openai_provider,
            moonshot_search: config.moonshot_search.clone(),
            moonshot_feature_enabled: config.features.enabled(Feature::KimiMoonshotWebSearch),
            settings: search_settings(config, web_search_mode),
        }
    }
}

fn search_settings(config: &Config, web_search_mode: WebSearchMode) -> SearchSettings {
    let web_search_config = config.web_search_config.as_ref();
    SearchSettings {
        user_location: web_search_config
            .and_then(|config| config.user_location.as_ref())
            .map(|location| ApproximateLocation {
                r#type: LocationType::Approximate,
                country: location.country.clone(),
                region: location.region.clone(),
                city: location.city.clone(),
                timezone: location.timezone.clone(),
            }),
        search_context_size: web_search_config
            .and_then(|config| config.search_context_size)
            .map(|size| match size {
                WebSearchContextSize::Low => SearchContextSize::Low,
                WebSearchContextSize::Medium => SearchContextSize::Medium,
                WebSearchContextSize::High => SearchContextSize::High,
            }),
        filters: web_search_config
            .and_then(|config| config.filters.as_ref())
            .map(|filters| SearchFilters {
                allowed_domains: filters.allowed_domains.clone(),
                blocked_domains: None,
            }),
        allowed_callers: Some(vec![AllowedCaller::Direct]),
        external_web_access: Some(external_web_access_for_mode(web_search_mode)),
        ..Default::default()
    }
}

fn external_web_access_for_mode(web_search_mode: WebSearchMode) -> ExternalWebAccess {
    match web_search_mode {
        WebSearchMode::Disabled | WebSearchMode::Cached => ExternalWebAccess::Boolean(false),
        WebSearchMode::Indexed => ExternalWebAccess::Mode(ExternalWebAccessMode::Indexed),
        WebSearchMode::Live => ExternalWebAccess::Boolean(true),
    }
}

impl ThreadLifecycleContributor<Config> for WebSearchExtension {
    fn on_thread_start<'a>(
        &'a self,
        input: ThreadStartInput<'a, Config>,
    ) -> ExtensionFuture<'a, ()> {
        Box::pin(async move {
            input
                .thread_store
                .insert(WebSearchExtensionConfig::from(input.config));
        })
    }
}

impl ConfigContributor<Config> for WebSearchExtension {
    fn on_config_changed(
        &self,
        _session_store: &ExtensionData,
        thread_store: &ExtensionData,
        _previous_config: &Config,
        new_config: &Config,
    ) {
        thread_store.insert(WebSearchExtensionConfig::from(new_config));
    }
}

impl ToolContributor for WebSearchExtension {
    fn tools(
        &self,
        session_store: &ExtensionData,
        thread_store: &ExtensionData,
    ) -> Vec<Arc<dyn codex_extension_api::ToolExecutor<codex_extension_api::ToolCall>>> {
        let Some(config) = thread_store.get::<WebSearchExtensionConfig>() else {
            return Vec::new();
        };
        if !config.available {
            return Vec::new();
        }

        vec![Arc::new(WebSearchTool {
            session_id: session_store.level_id().to_string(),
            primary_provider: create_model_provider(
                config.primary_provider.clone(),
                Some(self.auth_manager.clone()),
            ),
            openai_provider: create_model_provider(
                config.openai_provider.clone(),
                Some(self.auth_manager.clone()),
            ),
            moonshot_search: config.moonshot_search.clone(),
            moonshot_feature_enabled: config.moonshot_feature_enabled,
            settings: config.settings.clone(),
        })]
    }
}

pub fn install(registry: &mut ExtensionRegistryBuilder<Config>, auth_manager: Arc<AuthManager>) {
    let extension = Arc::new(WebSearchExtension { auth_manager });
    registry.thread_lifecycle_contributor(extension.clone());
    registry.config_contributor(extension.clone());
    registry.tool_contributor(extension);
}

#[cfg(test)]
mod tests {
    use codex_core::config::ConfigBuilder;
    use codex_extension_api::ExtensionData;
    use codex_extension_api::ExtensionRegistryBuilder;
    use codex_extension_api::ToolName;
    use codex_login::CodexAuth;
    use codex_model_provider_info::ModelProviderInfo;
    use codex_model_provider_info::WireApi;
    use codex_model_provider_info::create_oss_provider_with_base_url;
    use pretty_assertions::assert_eq;

    use super::AuthManager;
    use super::Config;
    use super::WebSearchExtensionConfig;
    use super::external_web_access_for_mode;
    use super::install;
    use crate::tool::RUN_TOOL_NAME;
    use crate::tool::WEB_NAMESPACE;
    use codex_api::ExternalWebAccess;
    use codex_api::ExternalWebAccessMode;
    use codex_protocol::config_types::WebSearchMode;

    #[test]
    fn external_web_access_preserves_legacy_values_until_indexed() {
        assert_eq!(
            [
                WebSearchMode::Disabled,
                WebSearchMode::Cached,
                WebSearchMode::Indexed,
                WebSearchMode::Live,
            ]
            .map(external_web_access_for_mode),
            [
                ExternalWebAccess::Boolean(false),
                ExternalWebAccess::Boolean(false),
                ExternalWebAccess::Mode(ExternalWebAccessMode::Indexed),
                ExternalWebAccess::Boolean(true),
            ]
        );
    }

    #[test]
    fn installed_extension_contributes_web_run_when_enabled() {
        let mut builder = ExtensionRegistryBuilder::<Config>::new();
        install(
            &mut builder,
            AuthManager::from_auth_for_testing(CodexAuth::from_api_key("dummy")),
        );
        let registry = builder.build();
        let session_store = ExtensionData::new("session");
        let thread_store = ExtensionData::new("11111111-1111-4111-8111-111111111111");
        thread_store.insert(WebSearchExtensionConfig {
            available: true,
            primary_provider: ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            openai_provider: ModelProviderInfo::create_openai_provider(/*base_url*/ None),
            moonshot_search: Default::default(),
            moonshot_feature_enabled: true,
            settings: Default::default(),
        });

        let tool_names = registry
            .tool_contributors()
            .iter()
            .flat_map(|contributor| contributor.tools(&session_store, &thread_store))
            .map(|tool| (tool.tool_name(), tool.supports_parallel_tool_calls()))
            .collect::<Vec<_>>();

        assert_eq!(
            tool_names,
            vec![(ToolName::namespaced(WEB_NAMESPACE, RUN_TOOL_NAME), true)]
        );
    }

    #[tokio::test]
    async fn ordinary_kimi_provider_is_available_for_web_run() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let mut config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .fallback_cwd(Some(codex_home.path().to_path_buf()))
            .build()
            .await
            .expect("test config should load");
        config.model = Some("gateway:k3".to_string());
        config.model_provider =
            create_oss_provider_with_base_url("https://api.moonshot.cn/v1", WireApi::Claude);

        let extension_config = WebSearchExtensionConfig::from(&config);

        assert!(extension_config.available);
        assert_eq!(extension_config.primary_provider, config.model_provider);
    }

    #[tokio::test]
    async fn selected_openai_provider_is_used_for_search() {
        let codex_home = tempfile::tempdir().expect("tempdir");
        let mut config = ConfigBuilder::default()
            .codex_home(codex_home.path().to_path_buf())
            .fallback_cwd(Some(codex_home.path().to_path_buf()))
            .build()
            .await
            .expect("test config should load");
        config.model_provider = ModelProviderInfo::create_openai_provider(Some(
            "https://search.example/api/codex".to_string(),
        ));

        let extension_config = WebSearchExtensionConfig::from(&config);

        assert_eq!(extension_config.openai_provider, config.model_provider);
    }
}
