//! Loads the model catalog for the current effective provider configuration.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use codex_config::ConfigLayerStack;
use codex_core::ThreadManager;
use codex_core::config::Config;
use codex_features::Feature;
use codex_model_provider_info::OPENAI_PROVIDER_ID;
use codex_models_manager::ModelOrigin;
use codex_models_manager::manager::RefreshStrategy;
use codex_models_manager::manager::SharedModelsManager;
use codex_protocol::openai_models::ModelPreset;
use codex_utils_absolute_path::AbsolutePathBuf;
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::config_manager::ConfigManager;
use crate::config_manager::StartupConfig;

/// Refreshes catalog routing without changing the routes retained by existing threads.
pub(crate) struct ModelCatalog {
    config_manager: ConfigManager,
    thread_manager: Arc<ThreadManager>,
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
        thread_manager: Arc<ThreadManager>,
    ) -> Self {
        let models_manager = thread_manager.get_models_manager();
        Self {
            config_manager,
            thread_manager,
            startup_cwd,
            state: Mutex::new(CatalogState {
                needs_reload: false,
                config,
                models_manager,
                refresh: Arc::new(Mutex::new(None)),
            }),
        }
    }

    /// Loads the same startup context used to reload the catalog after account setup.
    pub(crate) async fn load_startup_config_layers(&self) -> std::io::Result<ConfigLayerStack> {
        let cwd = self
            .startup_cwd
            .as_deref()
            .map(AbsolutePathBuf::from_absolute_path)
            .transpose()?;
        self.config_manager.load_config_layers(cwd).await
    }

    /// Marks catalog configuration stale after an account transition.
    pub(crate) async fn invalidate(&self) {
        self.state.lock().await.needs_reload = true;
    }

    pub(crate) async fn invalidate_if_current_provider(&self, provider_id: &str) {
        let mut state = self.state.lock().await;
        if state.config.model_provider_id == provider_id {
            state.needs_reload = true;
        }
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
                || config.features.enabled(Feature::RuntimeAgentInstances)
                    != state
                        .config
                        .features
                        .enabled(Feature::RuntimeAgentInstances)
                || config.runtime_auth_mode != state.config.runtime_auth_mode
            {
                state.models_manager = self.thread_manager.models_manager_for_config(&config);
                // A previous route must not delay discovery for its replacement.
                state.refresh = Arc::new(Mutex::new(None));
            }
            self.thread_manager.set_api_key_model_discovery_enabled(
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
        let models = self
            .list_models_from_snapshot(
                Arc::clone(&config),
                Arc::clone(&models_manager),
                refresh_strategy,
            )
            .await?;
        // The periodic worker also refreshes retained Work catalogs. Picker RPCs use
        // list_models_from_snapshot directly and do not wait on other originators.
        if refresh_strategy == RefreshStrategy::Online {
            models_manager
                .refresh_originator_catalogs(config.http_client_factory())
                .await;
        }
        Ok(models)
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
        let cached_models = if refresh_strategy == RefreshStrategy::OnlineIfUncached {
            if config.model_provider_id == OPENAI_PROVIDER_ID
                && (config.model_provider.supports_codex_backend_routes()
                    || config.model_provider.model_catalog_url.is_some())
            {
                models_manager
                    .cached_models_for_display_in_catalog_order()
                    .await
            } else {
                models_manager.cached_models_for_display().await
            }
        } else {
            None
        };
        if refresh_strategy == RefreshStrategy::OnlineIfUncached
            && let Some(models) = cached_models
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
            return Ok(Self::ordered_models(&config, &models_manager, refresh_strategy).await);
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
        let models = Self::ordered_models(&config, &models_manager, refresh_strategy).await;
        *refresh = Some(Instant::now());
        Ok(models)
    }

    async fn ordered_models(
        config: &Config,
        models_manager: &SharedModelsManager,
        refresh_strategy: RefreshStrategy,
    ) -> Vec<ModelPreset> {
        let catalog = models_manager
            .model_catalog_with_origins(refresh_strategy, config.http_client_factory())
            .await;
        // The first-party catalog supplies picker rows in display order. Bundled,
        // configured and third-party catalogs rely on the manager's priority order.
        let use_catalog_order = config.model_provider_id == OPENAI_PROVIDER_ID
            && (config.model_provider.supports_codex_backend_routes()
                || config.model_provider.model_catalog_url.is_some())
            && catalog
                .iter()
                .all(|entry| entry.origin == ModelOrigin::Provider);
        let positions = use_catalog_order.then(|| {
            catalog
                .iter()
                .enumerate()
                .map(|(index, entry)| (entry.slug.clone(), index))
                .collect::<HashMap<_, _>>()
        });
        let models = catalog
            .into_iter()
            .map(|entry| Arc::unwrap_or_clone(entry.model))
            .collect();
        let mut presets = models_manager.build_available_models(models);
        if let Some(positions) = positions {
            // The manager has filtered the models and marked the default using priority.
            presets.sort_by_key(|preset| positions[&preset.model]);
        }
        presets
    }
}

#[cfg(test)]
#[path = "model_catalog_tests.rs"]
mod tests;
