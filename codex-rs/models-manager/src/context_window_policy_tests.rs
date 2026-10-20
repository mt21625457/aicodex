//! 通过模型管理器验证窗口预算；临时 home 隔离真实账号和会话数据。
use super::TestModelsEndpoint;
use super::openai_manager_for_tests;
use super::static_manager_for_tests;
use crate::ModelsManagerConfig;
use crate::manager::ModelsManager;
use crate::model_info::{model_info_from_slug, with_config_overrides};
use codex_protocol::openai_models::ModelsResponse;

#[tokio::test]
async fn sol_bundled_catalog_uses_475k_without_leaking_budget_to_other_models() {
    let home = tempfile::tempdir().expect("临时 home");
    let manager = openai_manager_for_tests(
        home.path().to_path_buf(),
        TestModelsEndpoint::new(Vec::new()),
    );
    let config = ModelsManagerConfig::default();
    let before = manager.get_model_info("gpt-6-astra", &config).await;
    for _ in 0..2 {
        let sol = manager.get_model_info("gpt-6.1-sol", &config).await;
        assert_eq!(sol.context_window, Some(500_000));
        assert_eq!(sol.usable_context_window(), Some(475_000));
        assert_eq!(manager.get_model_info("gpt-6-astra", &config).await, before);
    }
    assert_eq!(config.model_context_window, None);
}

#[tokio::test]
async fn sol_missing_catalog_uses_500k_and_preserves_fallback_diagnostic() {
    let manager = static_manager_for_tests(ModelsResponse { models: Vec::new() });
    let sol = manager
        .get_model_info("gpt-6.1-sol", &ModelsManagerConfig::default())
        .await;
    assert_eq!(sol.context_window, Some(500_000));
    assert_eq!(sol.usable_context_window(), Some(475_000));
    assert!(sol.used_fallback_model_metadata);
}

#[test]
fn sol_catalog_defaults_preserve_provider_caps_and_other_metadata() {
    // 模拟远端旧目录、新目录和较小 provider 上限；只允许默认窗口变化。
    for (previous, maximum, expected) in [
        (272_000, Some(872_000), 500_000),
        (1_050_000, Some(1_050_000), 500_000),
        (272_000, None, 500_000),
        (200_000, Some(300_000), 300_000),
    ] {
        let mut source = model_info_from_slug("gpt-6.1-sol");
        source.context_window = Some(previous);
        source.max_context_window = maximum;
        source.max_output_tokens = Some(128_000);
        source.auto_compact_token_limit = Some(240_000);
        let mut expected_model = source.clone();
        expected_model.context_window = Some(expected);
        assert_eq!(
            with_config_overrides(source, &ModelsManagerConfig::default()),
            expected_model
        );
    }
}

#[tokio::test]
async fn sol_provider_catalog_applies_default_without_mutating_source() {
    let mut source = model_info_from_slug("gpt-6.1-sol");
    source.context_window = Some(272_000);
    source.max_context_window = Some(872_000);
    source.max_output_tokens = Some(128_000);
    source.auto_compact_token_limit = None;
    let manager = static_manager_for_tests(ModelsResponse {
        models: vec![source],
    });
    let config = ModelsManagerConfig::default();
    let sol = manager.get_model_info("gpt-6.1-sol", &config).await;
    assert_eq!(sol.usable_context_window(), Some(475_000));
    assert_eq!(sol.auto_compact_token_limit(), Some(372_000));
    // 模拟用户显式预算后重新解析默认预算，保护共享目录免受覆盖污染。
    let explicit = ModelsManagerConfig {
        model_context_window: Some(200_000),
        ..Default::default()
    };
    let overridden = manager.get_model_info("gpt-6.1-sol", &explicit).await;
    assert_eq!(overridden.context_window, Some(200_000));
    assert_eq!(manager.get_model_info("gpt-6.1-sol", &config).await, sol);
}

#[test]
fn explicit_sol_budget_keeps_precedence_and_provider_cap() {
    for (requested, expected) in [(200_000, 200_000), (900_000, 872_000)] {
        let mut source = model_info_from_slug("gpt-6.1-sol");
        source.max_context_window = Some(872_000);
        let config = ModelsManagerConfig {
            model_context_window: Some(requested),
            ..Default::default()
        };
        assert_eq!(
            with_config_overrides(source, &config).context_window,
            Some(expected)
        );
    }
}

#[test]
fn other_models_and_similar_slugs_keep_catalog_defaults() {
    for slug in [
        "gpt-6-astra",
        "gpt-6-sol",
        "gpt-6.1-sol-next",
        "custom/gpt-6.1-sol",
    ] {
        let source = model_info_from_slug(slug);
        assert_eq!(
            with_config_overrides(source.clone(), &ModelsManagerConfig::default()),
            source
        );
    }
}
