//! Read-only picker snapshots. Discovery and inference retain their normal refresh policy.

use super::*;

impl OpenAiModelsManager {
    #[expect(
        clippy::await_holding_invalid_type,
        reason = "Coalesce display-cache reads across disk I/O; inference never acquires this lock."
    )]
    pub(super) async fn display_snapshot(&self) -> Option<Vec<ModelPreset>> {
        // Command credentials may change on each resolution, so their existing discovery
        // path must resolve them before deciding whether a catalog belongs to this caller.
        if self.endpoint_client.has_command_auth()
            || (self.uses_api_key_auth()
                && (!self.supports_api_key_discovery()
                    || !self.api_key_model_discovery_enabled.load(Ordering::SeqCst)))
        {
            return None;
        }
        let mut display = self.display_models.write().await;
        let identity = self.endpoint_client.identity()?;
        {
            let current = self.remote_models.read().await;
            if self.remote_models_loaded.load(Ordering::Acquire)
                && current.identity.as_ref() == Some(&identity)
                && self.endpoint_client.identity().as_ref() == Some(&identity)
            {
                return Some(self.build_available_models(current.models.clone()));
            }
        }
        if let Some(entry) = display.as_ref()
            && entry.identity.as_ref() == Some(&identity)
            && self.endpoint_client.identity().as_ref() == Some(&identity)
        {
            return Some(self.build_available_models(entry.models.clone()));
        }
        let version = crate::client_version_to_whole();
        let mut entry = self
            .cache
            .as_ref()?
            .load_for_display(&version)
            .await
            .ok()??;
        if entry.identity.as_ref() != Some(&identity)
            || entry.client_version.as_deref() != Some(version.as_str())
            || self.endpoint_client.identity().as_ref() != Some(&identity)
        {
            return None;
        }
        let current = self.remote_models.read().await;
        if self.endpoint_client.identity().as_ref() != Some(&identity) {
            return None;
        }
        // Prefer a response that arrived during disk I/O, without publishing stale
        // display metadata into the catalog used for inference and ETag validation.
        if self.remote_models_loaded.load(Ordering::Acquire)
            && current.identity.as_ref() == Some(&identity)
        {
            return Some(self.build_available_models(current.models.clone()));
        }
        entry.models = self.merge_remote_models(entry.models);
        let models = self.build_available_models(entry.models.clone());
        *display = Some(entry);
        Some(models)
    }

    pub(super) fn merge_remote_models(&self, remote_models: Vec<ModelInfo>) -> Vec<ModelInfo> {
        // Visible ChatGPT and OpenAI API-key catalogs are authoritative.
        let remote_only = remote_models
            .iter()
            .any(|model| model.visibility == ModelVisibility::List)
            && (self.supports_api_key_discovery()
                || self.auth_manager.as_ref().is_some_and(|auth_manager| {
                    auth_manager
                        .auth_mode()
                        .is_some_and(AuthMode::has_chatgpt_account)
                }));
        if remote_only {
            return remote_models;
        }
        let Some(mut models) = self.catalog_source.fallback_models() else {
            return remote_models;
        };
        for model in remote_models {
            if let Some(index) = models
                .iter()
                .position(|existing| existing.slug == model.slug)
            {
                models[index] = model;
            } else {
                models.push(model);
            }
        }
        models
    }
}
