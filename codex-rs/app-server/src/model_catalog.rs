//! Loads the model catalog for the current effective provider configuration.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use codex_core::config::Config;
use codex_features::Feature;
use codex_login::AuthManager;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::openai_models::ModelPreset;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::config_manager::ConfigManager;
use crate::config_manager::StartupConfig;

/// Refreshes catalog routing without changing the routes retained by existing threads.
pub(crate) struct ModelCatalog {
    config_manager: ConfigManager,
    auth_manager: Arc<AuthManager>,
    startup_cwd: Option<PathBuf>,
    state: Mutex<CatalogState>,
}

struct CatalogState {
    needs_reload: bool,
    config: Arc<Config>,
    models_manager: SharedModelsManager,
    refresh: Arc<Mutex<Option<Instant>>>,
}

impl ModelCatalog {
    pub(crate) fn new(
        config_manager: ConfigManager,
        startup_cwd: Option<PathBuf>,
        config: Arc<Config>,
        models_manager: SharedModelsManager,
        auth_manager: Arc<AuthManager>,
    ) -> Self {
        Self {
            config_manager,
            auth_manager,
            startup_cwd,
            state: Mutex::new(CatalogState {
                needs_reload: false,
                config,
                models_manager,
                refresh: Arc::new(Mutex::new(None)),
            }),
        }
    }

    /// Marks catalog configuration stale after an account transition.
    pub(crate) async fn invalidate(&self) {
        self.state.lock().await.needs_reload = true;
    }

    pub(crate) async fn set_api_key_model_discovery_enabled(&self, enabled: bool) {
        self.state
            .lock()
            .await
            .models_manager
            .set_api_key_model_discovery_enabled(enabled);
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "catalog reloads are serialized with account invalidation; model fetching holds no lock"
    )]
    pub(crate) async fn current(&self) -> std::io::Result<(Arc<Config>, SharedModelsManager)> {
        let mut state = self.state.lock().await;
        if state.needs_reload {
            // Keep the large config-loading future off callers' stacks.
            let config = Box::pin(
                self.config_manager
                    .load_startup_config(self.startup_cwd.clone()),
            )
            .await?;
            let StartupConfig {
                config,
                used_defaults,
            } = config;
            if config.model_provider_id != state.config.model_provider_id
                || config.model_provider != state.config.model_provider
                || config.model_catalog != state.config.model_catalog
            {
                state.models_manager =
                    codex_core::build_models_manager(&config, Arc::clone(&self.auth_manager));
                // A previous route must not delay discovery for its replacement.
                state.refresh = Arc::new(Mutex::new(None));
            }
            state.models_manager.set_api_key_model_discovery_enabled(
                config.features.enabled(Feature::ApiKeyModelDiscovery),
            );
            state.config = Arc::new(config);
            // Try again on the next access so a temporary fallback cannot pin the provider.
            state.needs_reload = used_defaults;
        }
        Ok((Arc::clone(&state.config), Arc::clone(&state.models_manager)))
    }

    pub(crate) async fn list_models(
        &self,
        refresh_strategy: RefreshStrategy,
    ) -> std::io::Result<Vec<ModelPreset>> {
        let (config, models_manager) = self.current().await?;
        self.list_models_from_snapshot(config, models_manager, refresh_strategy)
            .await
    }

    /// Shares discovery work only for the manager represented by this snapshot.
    /// A retained snapshot of an old route must not contend with its replacement.
    async fn refresh_for_snapshot(
        &self,
        models_manager: &SharedModelsManager,
    ) -> Arc<Mutex<Option<Instant>>> {
        let state = self.state.lock().await;
        if Arc::ptr_eq(&state.models_manager, models_manager) {
            Arc::clone(&state.refresh)
        } else {
            Arc::new(Mutex::new(None))
        }
    }

    #[expect(
        clippy::await_holding_invalid_type,
        reason = "Serialize snapshot discovery and revalidate policy after queued requests acquire the lock."
    )]
    pub(crate) async fn list_models_from_snapshot(
        &self,
        config: Arc<Config>,
        models_manager: SharedModelsManager,
        refresh_strategy: RefreshStrategy,
    ) -> std::io::Result<Vec<ModelPreset>> {
        // Keep the existing policy check even for a warm catalog. A policy
        // edit alone must not make the retained provider silently permissible.
        self.config_manager
            .check_thread_model_provider(&config)
            .await?;
        let refresh_lock = self.refresh_for_snapshot(&models_manager).await;
        if refresh_strategy == RefreshStrategy::OnlineIfUncached
            && let Some(models) = models_manager.cached_models_for_display().await
        {
            if let Ok(mut refresh) = Arc::clone(&refresh_lock).try_lock_owned()
                && refresh
                    .is_none_or(|last| last.elapsed() >= Duration::from_secs(/*secs*/ 30))
            {
                // Reserve before spawning so concurrent picker requests share one refresh.
                *refresh = Some(Instant::now());
                let config_manager = self.config_manager.clone();
                tokio::spawn(async move {
                    // Recheck policy at execution time, including after config changes.
                    if let Err(err) = config_manager.check_thread_model_provider(&config).await {
                        tracing::warn!(error_kind = ?err.kind(), "model catalog refresh blocked by provider requirements");
                    } else {
                        models_manager
                            .list_models(
                                RefreshStrategy::OnlineIfUncached,
                                config.http_client_factory(),
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
            return Ok(models_manager
                .list_models(refresh_strategy, config.http_client_factory())
                .await);
        }
        // Serialize cold discovery with the periodic worker and picker refreshes.
        let mut refresh = match refresh_lock.try_lock() {
            Ok(refresh) => refresh,
            Err(_) => {
                let refresh = refresh_lock.lock().await;
                // Requirements may have changed while another discovery held the lock.
                self.config_manager
                    .check_thread_model_provider(&config)
                    .await?;
                refresh
            }
        };
        let models = models_manager
            .list_models(refresh_strategy, config.http_client_factory())
            .await;
        *refresh = Some(Instant::now());
        Ok(models)
    }
}

#[cfg(test)]
#[path = "model_catalog_tests.rs"]
mod tests;
