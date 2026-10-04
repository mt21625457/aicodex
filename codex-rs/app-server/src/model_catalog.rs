//! Gates access to the retained startup model catalog on current managed provider requirements.

use std::sync::Arc;
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::Instant;

use codex_core::config::Config;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::openai_models::ModelPreset;

use crate::config_manager::ConfigManager;

/// Checks the retained catalog route before model/list and background refreshes.
pub(crate) struct ModelCatalog {
    config_manager: ConfigManager,
    config: Arc<Config>,
    models_manager: SharedModelsManager,
    refresh: Arc<Mutex<Option<Instant>>>,
}

impl ModelCatalog {
    pub(crate) fn new(
        config_manager: ConfigManager,
        config: Arc<Config>,
        models_manager: SharedModelsManager,
    ) -> Self {
        Self {
            config_manager,
            config,
            models_manager,
            refresh: Arc::new(Mutex::new(None)),
        }
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "Serialize catalog refresh I/O and revalidate policy after queued requests acquire the lock."
    )]
    pub(crate) async fn list_models(
        self: &Arc<Self>,
        refresh_strategy: RefreshStrategy,
    ) -> std::io::Result<Vec<ModelPreset>> {
        // Check before consulting even a warm cache, just as turn admission checks
        // the retained session route before using it.
        self.config_manager
            .check_thread_model_provider(&self.config)
            .await?;
        if refresh_strategy == RefreshStrategy::OnlineIfUncached
            && let Some(models) = self.models_manager.cached_models_for_display().await
        {
            if let Ok(mut refresh) = Arc::clone(&self.refresh).try_lock_owned()
                && refresh
                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(/*secs*/ 30))
            {
                // Reserve before spawning so concurrent picker requests share one refresh.
                *refresh = Some(Instant::now());
                let catalog = Arc::clone(self);
                tokio::spawn(async move {
                    // Recheck policy at execution time, including after config changes.
                    if let Err(err) = catalog
                        .config_manager
                        .check_thread_model_provider(&catalog.config)
                        .await
                    {
                        tracing::warn!(error_kind = ?err.kind(), "model catalog refresh blocked by provider requirements");
                    } else {
                        catalog
                            .models_manager
                            .list_models(
                                RefreshStrategy::OnlineIfUncached,
                                catalog.config.http_client_factory(),
                            )
                            .await;
                    }
                    // Keep the previous display snapshot on failure and back off from completion.
                    *refresh = Some(Instant::now());
                });
            }
            return Ok(models);
        }
        if refresh_strategy == RefreshStrategy::Offline {
            return Ok(self
                .models_manager
                .list_models(refresh_strategy, self.config.http_client_factory())
                .await);
        }
        // Serialize cold discovery with the periodic worker and picker refreshes.
        let mut refresh = match self.refresh.try_lock() {
            Ok(refresh) => refresh,
            Err(_) => {
                let refresh = self.refresh.lock().await;
                // Requirements may have changed while another discovery held the lock.
                self.config_manager
                    .check_thread_model_provider(&self.config)
                    .await?;
                refresh
            }
        };
        let models = self
            .models_manager
            .list_models(refresh_strategy, self.config.http_client_factory())
            .await;
        *refresh = Some(Instant::now());
        Ok(models)
    }
}

#[cfg(test)]
#[path = "model_catalog_tests.rs"]
mod tests;
