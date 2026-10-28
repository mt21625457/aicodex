pub mod cache;
pub mod collaboration_mode_presets;
pub(crate) mod config;
pub mod manager;
pub mod model_info;
pub mod model_presets;
mod originator;
pub mod test_support;

pub use codex_protocol::auth::AuthMode;
pub use config::ModelsManagerConfig;
pub use originator::ModelCatalogOriginator;

/// Load the bundled model catalog shipped with `codex-models-manager`.
pub fn bundled_models_response()
-> std::result::Result<codex_protocol::openai_models::ModelsResponse, serde_json::Error> {
    bundled_models_response_ref().cloned()
}

/// Borrow the immutable bundled catalog without copying unrelated model metadata.
pub fn bundled_models_response_ref()
-> std::result::Result<&'static codex_protocol::openai_models::ModelsResponse, serde_json::Error> {
    // 仅缓存编译时内置目录；远端目录、账户身份和展示缓存继续由 manager 独立管理。
    // 不缓存解析错误，以保留原来的 serde_json::Error 返回类型与失败行为。
    static BUNDLED: std::sync::OnceLock<codex_protocol::openai_models::ModelsResponse> =
        std::sync::OnceLock::new();
    if let Some(models) = BUNDLED.get() {
        return Ok(models);
    }
    let parsed = serde_json::from_str(include_str!("../models.json"))?;
    Ok(BUNDLED.get_or_init(|| parsed))
}

/// Convert the client version string to a whole version string (e.g. "1.2.3-alpha.4" -> "1.2.3").
pub fn client_version_to_whole() -> String {
    format!(
        "{}.{}.{}",
        env!("CARGO_PKG_VERSION_MAJOR"),
        env!("CARGO_PKG_VERSION_MINOR"),
        env!("CARGO_PKG_VERSION_PATCH")
    )
}

#[cfg(test)]
mod bundled_catalog_tests {
    #[test]
    fn borrowed_catalog_is_shared_but_selected_models_remain_independent() {
        let first = super::bundled_models_response_ref().expect("load borrowed catalog");
        let second = super::bundled_models_response_ref().expect("reload borrowed catalog");
        assert!(std::ptr::eq(first, second));
        let mut selected = first.models.first().expect("non-empty catalog").clone();
        let original = selected.clone();
        selected.display_name.push_str(" changed by caller");
        assert_eq!(second.models.first(), Some(&original));
        assert_eq!(super::bundled_models_response().unwrap(), *first);
    }

    #[test]
    fn cached_catalog_matches_source_and_returns_independent_values() {
        let source: codex_protocol::openai_models::ModelsResponse =
            serde_json::from_str(include_str!("../models.json")).expect("parse source");
        let mut first = super::bundled_models_response().expect("load catalog");
        assert_eq!(
            serde_json::to_value(&first).unwrap(),
            serde_json::to_value(&source).unwrap()
        );
        // 修改调用方拥有的目录不得污染后续账户或 provider 的目录初始化。
        first.models.clear();
        let second = super::bundled_models_response().expect("reload catalog");
        assert_eq!(
            serde_json::to_value(&second).unwrap(),
            serde_json::to_value(&source).unwrap()
        );
    }
}
