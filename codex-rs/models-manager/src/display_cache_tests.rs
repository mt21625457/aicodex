use super::*;
use pretty_assertions::assert_eq;

#[tokio::test]
async fn explicit_provider_display_cache_does_not_add_bundled_models() {
    for models in [
        vec![],
        vec![remote_model_with_visibility(
            "hidden-provider-model",
            "Hidden provider model",
            /*priority*/ 0,
            "hide",
        )],
    ] {
        let home = tempdir().unwrap();
        let endpoint = TestModelsEndpoint::new(vec![]);
        let entry = ModelsCacheEntry {
            fetched_at: Utc::now() - chrono::Duration::hours(1),
            etag: None,
            client_version: Some(crate::client_version_to_whole()),
            identity: endpoint.identity(),
            models,
        };
        let cache =
            FileModelsCache::new(home.path().join(MODEL_CACHE_FILE), DEFAULT_MODEL_CACHE_TTL);
        cache.store(&entry).await.unwrap();
        let manager = openai_manager_for_tests(home.path().to_path_buf(), endpoint.clone())
            .with_provider_catalog();

        assert_eq!(
            manager.cached_models_for_display().await,
            Some(manager.build_available_models(entry.models))
        );
        assert_eq!(manager.get_remote_models().await, Vec::<ModelInfo>::new());
        assert_eq!(endpoint.fetch_count(), 0);
    }
}

#[tokio::test]
async fn older_cache_update_preserves_newer_network_catalog() {
    let home = tempdir().unwrap();
    let endpoint = TestModelsEndpoint::new(vec![]);
    let manager = openai_manager_for_tests(home.path().to_path_buf(), endpoint.clone());
    let latest = ModelsCacheEntry {
        fetched_at: Utc::now(),
        etag: Some("latest-etag".to_string()),
        client_version: Some(crate::client_version_to_whole()),
        identity: endpoint.identity(),
        models: vec![remote_model("latest", "Latest", /*priority*/ 0)],
    };
    let older = ModelsCacheEntry {
        fetched_at: latest.fetched_at - chrono::Duration::minutes(1),
        etag: Some("older-etag".to_string()),
        models: vec![remote_model("older", "Older", /*priority*/ 0)],
        ..latest.clone()
    };
    assert!(
        manager
            .apply_remote_models(latest.clone(), CatalogUpdateSource::Network)
            .await
    );
    assert!(
        manager
            .apply_remote_models(older, CatalogUpdateSource::Cache)
            .await
    );
    assert_eq!(*manager.remote_models.read().await, latest);
}

#[tokio::test]
async fn expired_display_cache_survives_restart_without_fetching() {
    let home = tempdir().unwrap();
    let endpoint = TestModelsEndpoint::new(vec![]);
    let model = remote_model(
        "cached-gateway-model",
        "Cached gateway model",
        /*priority*/ 0,
    );
    let entry = ModelsCacheEntry {
        fetched_at: Utc::now() - chrono::Duration::hours(1),
        etag: None,
        client_version: Some(crate::client_version_to_whole()),
        identity: endpoint.identity(),
        models: vec![model],
    };
    let cache = FileModelsCache::new(home.path().join(MODEL_CACHE_FILE), DEFAULT_MODEL_CACHE_TTL);
    cache.store(&entry).await.unwrap();
    assert!(
        cache
            .load(&crate::client_version_to_whole())
            .await
            .unwrap()
            .is_none()
    );
    let manager = openai_manager_for_tests(home.path().to_path_buf(), endpoint.clone());
    let inference_catalog = manager.get_remote_models().await;
    let expected = Some(manager.build_available_models(entry.models));
    assert_eq!(manager.cached_models_for_display().await, expected);
    assert_eq!(manager.get_remote_models().await, inference_catalog);
    assert_eq!(manager.try_get_remote_models().unwrap(), inference_catalog);
    assert_eq!(
        manager
            .raw_model_catalog(RefreshStrategy::Offline, DEFAULT_HTTP_CLIENT_FACTORY)
            .await
            .models,
        inference_catalog
    );
    // A later missing disk entry must not discard a valid in-memory display snapshot.
    std::fs::remove_file(home.path().join(MODEL_CACHE_FILE)).unwrap();
    assert_eq!(manager.cached_models_for_display().await, expected);
    assert_eq!(endpoint.fetch_count(), 0);
}

#[tokio::test]
async fn display_cache_does_not_satisfy_etag_revalidation() {
    let home = tempdir().unwrap();
    let updated = remote_model("updated-model", "Updated model", /*priority*/ 0);
    let endpoint = TestModelsEndpoint::new(vec![vec![updated.clone()]]);
    let cache = FileModelsCache::new(home.path().join(MODEL_CACHE_FILE), DEFAULT_MODEL_CACHE_TTL);
    cache
        .store(&ModelsCacheEntry {
            fetched_at: Utc::now() - chrono::Duration::hours(1),
            etag: Some("cached-etag".to_string()),
            client_version: Some(crate::client_version_to_whole()),
            identity: endpoint.identity(),
            models: vec![remote_model(
                "cached-model",
                "Cached model",
                /*priority*/ 0,
            )],
        })
        .await
        .unwrap();
    let manager = openai_manager_for_tests(home.path().to_path_buf(), endpoint.clone());
    assert!(manager.cached_models_for_display().await.is_some());
    manager
        .refresh_if_new_etag("cached-etag".to_string(), DEFAULT_HTTP_CLIENT_FACTORY)
        .await;
    assert_eq!(endpoint.fetch_count(), 1);
    assert_eq!(manager.get_remote_models().await, vec![updated.clone()]);
    assert_eq!(
        manager.cached_models_for_display().await,
        Some(manager.build_available_models(vec![updated]))
    );
}

#[tokio::test]
async fn display_cache_rejects_other_identities_and_versions() {
    let home = tempdir().unwrap();
    let endpoint = TestModelsEndpoint::new(vec![]);
    let manager = openai_manager_for_tests(home.path().to_path_buf(), endpoint.clone());
    let cache = FileModelsCache::new(home.path().join(MODEL_CACHE_FILE), DEFAULT_MODEL_CACHE_TTL);
    for (identity, version) in [
        (
            Some("other-account".to_string()),
            Some(crate::client_version_to_whole()),
        ),
        (endpoint.identity(), Some("different-version".to_string())),
        (None, Some(crate::client_version_to_whole())),
    ] {
        cache
            .store(&ModelsCacheEntry {
                fetched_at: Utc::now(),
                etag: None,
                client_version: version,
                identity,
                models: vec![model_info::model_info_from_slug("private-model")],
            })
            .await
            .unwrap();
        assert_eq!(manager.cached_models_for_display().await, None);
    }
    assert_eq!(endpoint.fetch_count(), 0);
}

#[tokio::test]
async fn display_cache_does_not_treat_bundled_defaults_as_a_fetched_catalog() {
    let home = tempdir().unwrap();
    let endpoint = TestModelsEndpoint::new(vec![]);
    let manager = openai_manager_for_tests(home.path().to_path_buf(), endpoint.clone());
    assert_eq!(manager.cached_models_for_display().await, None);
    assert_eq!(endpoint.fetch_count(), 0);
}

#[tokio::test]
async fn display_cache_respects_discovery_disable_and_auth_changes() {
    let home = tempdir().unwrap();
    let auth =
        AuthManager::from_auth_for_testing(CodexAuth::create_dummy_chatgpt_auth_for_testing());
    let endpoint = TestAuthAwareModelsEndpoint::new(
        Some(auth.clone()),
        vec![vec![remote_model(
            "account-model",
            "Account model",
            /*priority*/ 0,
        )]],
    );
    let manager = openai_manager_for_tests_with_auth(
        home.path().to_path_buf(),
        endpoint.clone(),
        Some(auth.clone()),
    );
    manager
        .raw_model_catalog(RefreshStrategy::Online, DEFAULT_HTTP_CLIENT_FACTORY)
        .await;
    assert!(manager.cached_models_for_display().await.is_some());
    auth.set_external_auth(Arc::new(TestExternalApiKeyAuth))
        .await
        .unwrap();
    assert_eq!(manager.cached_models_for_display().await, None);
    manager.set_api_key_model_discovery_enabled(/*enabled*/ true);
    // Even with discovery enabled, the preceding ChatGPT catalog belongs to another identity.
    assert_eq!(manager.cached_models_for_display().await, None);
    manager
        .raw_model_catalog(RefreshStrategy::Online, DEFAULT_HTTP_CLIENT_FACTORY)
        .await;
    assert!(manager.cached_models_for_display().await.is_some());
    manager.set_api_key_model_discovery_enabled(/*enabled*/ false);
    assert_eq!(manager.cached_models_for_display().await, None);
    assert_eq!(endpoint.fetch_count(), 2);
}
