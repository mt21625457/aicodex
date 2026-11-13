use super::*;

fn catalog_with_models(
    config_manager: ConfigManager,
    config: Arc<Config>,
    models: SharedModelsManager,
) -> Arc<ModelCatalog> {
    struct TestCatalogFactory {
        initial_provider: codex_model_provider_info::ModelProviderInfo,
        initial_models: SharedModelsManager,
        fallback: codex_models_manager::manager::ModelCatalogSettings,
    }
    impl codex_model_provider::ModelCatalogFactory for TestCatalogFactory {
        fn build(
            &self,
            provider: codex_model_provider::SharedModelProvider,
            catalog: Option<codex_protocol::openai_models::ModelsResponse>,
            discovery: bool,
        ) -> SharedModelsManager {
            if provider.info() == &self.initial_provider && catalog.is_none() {
                Arc::clone(&self.initial_models)
            } else {
                self.fallback.build(provider, catalog, discovery)
            }
        }

        fn set_api_key_model_discovery_enabled(&self, enabled: bool) {
            self.initial_models
                .set_api_key_model_discovery_enabled(enabled);
            self.fallback.set_api_key_model_discovery_enabled(enabled);
        }
    }
    let startup = codex_core::build_startup_models(
        &config,
        AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing()),
        uuid::Uuid::new_v4().to_string(),
        Arc::new(TestCatalogFactory {
            initial_provider: config.model_provider.clone(),
            initial_models: models,
            fallback: codex_models_manager::manager::ModelCatalogSettings::disk(
                config.codex_home.clone(),
            ),
        }),
    );
    let manager = Arc::new(codex_core::ThreadManager::new(
        &config,
        startup,
        codex_core::CodexAppsToolsCache::default(),
        codex_protocol::protocol::SessionSource::Exec,
        Arc::new(codex_exec_server::EnvironmentManager::default_for_tests()),
        codex_extension_api::empty_extension_registry(),
        Arc::new(codex_core::test_support::EmptyUserInstructionsProvider),
        None,
        codex_core::passthrough_image_store(),
        codex_core::ThreadStoreBuilder::from_config(&config, None).build(),
        None,
        None,
        None,
    ));
    Arc::new(ModelCatalog::new(config_manager, None, config, manager))
}
use codex_http_client::HttpClientFactory;
use codex_login::AuthManager;
use codex_login::CodexAuth;
use codex_models_manager::cache::ModelsCacheEntry;
use codex_models_manager::manager::ModelsEndpointClient;
use codex_models_manager::manager::ModelsEndpointFuture;
use codex_models_manager::manager::ModelsEndpointResponse;
use codex_models_manager::manager::OpenAiModelsManager;
use codex_protocol::error::CodexErr;
use codex_protocol::error::Result as CoreResult;
use codex_protocol::openai_models::ModelInfo;
use codex_protocol::openai_models::ModelVisibility;
use pretty_assertions::assert_eq;
use std::sync::atomic::AtomicBool;
use std::sync::atomic::AtomicUsize;
use std::sync::atomic::Ordering;
use tokio::sync::Notify;

#[derive(Debug, Default)]
struct SlowEndpoint {
    requests: AtomicUsize,
    fail: AtomicBool,
    started: Notify,
    release: Notify,
    command_auth: bool,
    display_checked: Notify,
}

impl ModelsEndpointClient for SlowEndpoint {
    fn identity(&self) -> Option<String> {
        Some("gateway-account".to_string())
    }

    fn has_command_auth(&self) -> bool {
        if self.command_auth {
            self.display_checked.notify_one();
        }
        self.command_auth
    }

    fn uses_codex_backend(&self) -> ModelsEndpointFuture<'_, bool> {
        Box::pin(std::future::ready(true))
    }

    fn list_models<'a>(
        &'a self,
        _client_version: &'a str,
        _http_client_factory: HttpClientFactory,
    ) -> ModelsEndpointFuture<'a, CoreResult<ModelsEndpointResponse>> {
        Box::pin(async move {
            self.requests.fetch_add(1, Ordering::SeqCst);
            self.started.notify_one();
            self.release.notified().await;
            if self.fail.load(Ordering::SeqCst) {
                return Err(CodexErr::RequestTimeout);
            }
            Ok(ModelsEndpointResponse {
                models: vec![visible_model("updated-model")],
                etag: None,
                identity: self.identity().unwrap(),
            })
        })
    }
}

#[tokio::test]
#[expect(
    clippy::await_holding_invalid_type,
    reason = "Hold the refresh lock while the request queues to test policy changes during contention."
)]
async fn queued_discovery_rechecks_provider_requirements() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Arc::new(SlowEndpoint {
        command_auth: true,
        ..Default::default()
    });
    let config_manager = ConfigManager::new_for_tests(
        home.path().to_path_buf(),
        Vec::new(),
        codex_config::LoaderOverrides::with_managed_config_path_for_tests(
            home.path().join("managed_config.toml"),
        ),
        codex_config::CloudConfigBundleLoader::default(),
    );
    let config = Arc::new(config_manager.load_non_project_config().await.unwrap());
    let models_manager = Arc::new(OpenAiModelsManager::new(
        home.path().to_path_buf(),
        endpoint.clone(),
        /*auth_manager*/ None,
    ));
    let catalog = catalog_with_models(config_manager, config, models_manager);
    let refresh_lock = Arc::clone(&catalog.state.lock().await.refresh);
    let refresh = refresh_lock.lock().await;
    let request = tokio::spawn({
        let catalog = Arc::clone(&catalog);
        async move { catalog.list_models(RefreshStrategy::OnlineIfUncached).await }
    });
    // Command auth returns no display snapshot without yielding, so this signal
    // means discovery has passed its first policy check and queued on the lock.
    tokio::time::timeout(
        Duration::from_secs(/*secs*/ 2),
        endpoint.display_checked.notified(),
    )
    .await
    .unwrap();
    std::fs::write(
        home.path().join("requirements.toml"),
        "model_provider = 'ollama'",
    )
    .unwrap();
    drop(refresh);
    let error = tokio::time::timeout(Duration::from_secs(/*secs*/ 2), request)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::PermissionDenied);
    assert_eq!(endpoint.requests.load(Ordering::SeqCst), 0);
}

fn visible_model(slug: &str) -> ModelInfo {
    ModelInfo {
        visibility: ModelVisibility::List,
        ..codex_models_manager::model_info::model_info_from_slug(slug)
    }
}

async fn cached_catalog(home: &std::path::Path, endpoint: Arc<SlowEndpoint>) -> Arc<ModelCatalog> {
    let entry = ModelsCacheEntry {
        fetched_at: chrono::Utc::now() - chrono::Duration::hours(1),
        etag: None,
        client_version: Some(codex_models_manager::client_version_to_whole()),
        identity: endpoint.identity(),
        models: vec![visible_model("cached-model")],
    };
    std::fs::write(
        home.join("models_cache.json"),
        serde_json::to_vec(&entry).unwrap(),
    )
    .unwrap();
    let config_manager = ConfigManager::without_managed_config_for_tests(home.to_path_buf());
    let config = Arc::new(config_manager.load_non_project_config().await.unwrap());
    let models_manager = Arc::new(OpenAiModelsManager::new(
        home.to_path_buf(),
        endpoint,
        Some(AuthManager::from_auth_for_testing(
            CodexAuth::create_dummy_chatgpt_auth_for_testing(),
        )),
    ));
    catalog_with_models(config_manager, config, models_manager)
}

#[tokio::test]
async fn stale_catalog_returns_while_refresh_is_blocked_and_coalesces_requests() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Arc::new(SlowEndpoint::default());
    let catalog = cached_catalog(home.path(), endpoint.clone()).await;
    let models_manager = catalog.current().await.unwrap().1;
    let expected = models_manager.cached_models_for_display().await.unwrap();
    let first = tokio::time::timeout(
        Duration::from_secs(/*secs*/ 2),
        catalog.list_models(RefreshStrategy::OnlineIfUncached),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(first, expected);
    tokio::time::timeout(Duration::from_secs(/*secs*/ 2), endpoint.started.notified())
        .await
        .unwrap();
    // These reads finish even though the endpoint cannot complete until we release it.
    for _ in 0..5 {
        assert_eq!(
            tokio::time::timeout(
                Duration::from_secs(/*secs*/ 2),
                catalog.list_models(RefreshStrategy::OnlineIfUncached),
            )
            .await
            .unwrap()
            .unwrap(),
            expected
        );
    }
    assert_eq!(endpoint.requests.load(Ordering::SeqCst), 1);
    endpoint.release.notify_one();
    let refresh_lock = Arc::clone(&catalog.state.lock().await.refresh);
    let refresh = tokio::time::timeout(Duration::from_secs(/*secs*/ 2), refresh_lock.lock())
        .await
        .unwrap();
    drop(refresh);
    let updated = catalog
        .list_models(RefreshStrategy::OnlineIfUncached)
        .await
        .unwrap();
    assert_eq!(
        updated,
        models_manager.build_available_models(vec![visible_model("updated-model")])
    );
    assert_eq!(endpoint.requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn failed_refresh_preserves_catalog_and_backs_off() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Arc::new(SlowEndpoint::default());
    endpoint.fail.store(true, Ordering::SeqCst);
    let catalog = cached_catalog(home.path(), endpoint.clone()).await;
    let expected = catalog
        .list_models(RefreshStrategy::OnlineIfUncached)
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(/*secs*/ 2), endpoint.started.notified())
        .await
        .unwrap();
    endpoint.release.notify_one();
    let refresh_lock = Arc::clone(&catalog.state.lock().await.refresh);
    let refresh = tokio::time::timeout(Duration::from_secs(/*secs*/ 2), refresh_lock.lock())
        .await
        .unwrap();
    drop(refresh);
    for _ in 0..5 {
        assert_eq!(
            catalog
                .list_models(RefreshStrategy::OnlineIfUncached)
                .await
                .unwrap(),
            expected
        );
    }
    assert_eq!(endpoint.requests.load(Ordering::SeqCst), 1);
    // Expire only the retry deadline; no wall-clock sleep or clock mutation is needed.
    *refresh_lock.lock().await = Some(Instant::now() - Duration::from_secs(/*secs*/ 31));
    assert_eq!(
        catalog
            .list_models(RefreshStrategy::OnlineIfUncached)
            .await
            .unwrap(),
        expected
    );
    tokio::time::timeout(Duration::from_secs(/*secs*/ 2), endpoint.started.notified())
        .await
        .unwrap();
    assert_eq!(endpoint.requests.load(Ordering::SeqCst), 2);
    endpoint.release.notify_one();
    let refresh_lock = Arc::clone(&catalog.state.lock().await.refresh);
    let refresh = tokio::time::timeout(Duration::from_secs(/*secs*/ 2), refresh_lock.lock())
        .await
        .unwrap();
    drop(refresh);
}

/// Reloads must neither replace a same-route refresh guard nor inherit an old route's backoff.
#[tokio::test]
async fn reload_scopes_refresh_state_to_the_current_model_manager() {
    let home = tempfile::tempdir().unwrap();
    let endpoint = Arc::new(SlowEndpoint::default());
    let catalog = cached_catalog(home.path(), endpoint).await;
    let (old_config, old_models) = catalog.current().await.unwrap();
    let expected_old_catalog = old_models
        .list_models(RefreshStrategy::Offline, old_config.http_client_factory())
        .await;
    let old_refresh = catalog.refresh_for_snapshot(&old_models).await;
    let old_guard = old_refresh.clone().lock_owned().await;

    catalog.invalidate().await;
    let (_, same_models) = catalog.current().await.unwrap();
    assert!(Arc::ptr_eq(&same_models, &old_models));
    assert!(Arc::ptr_eq(
        &catalog.refresh_for_snapshot(&same_models).await,
        &old_refresh,
    ));

    std::fs::write(home.path().join("config.toml"), "model_provider = 'ollama'").unwrap();
    catalog.invalidate().await;
    let (new_config, new_models) = catalog.current().await.unwrap();
    assert_eq!(new_config.model_provider_id, "ollama");
    assert!(!Arc::ptr_eq(&new_models, &old_models));
    let new_refresh = catalog.refresh_for_snapshot(&new_models).await;
    assert!(!Arc::ptr_eq(&new_refresh, &old_refresh));
    assert_eq!(*new_refresh.try_lock().unwrap(), None);
    // A request already holding the old routing snapshot still sees its old catalog.
    let old_catalog = catalog
        .list_models_from_snapshot(old_config, old_models, RefreshStrategy::Offline)
        .await
        .unwrap();
    assert_eq!(old_catalog, expected_old_catalog);
    drop(old_guard);
}
