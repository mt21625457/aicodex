use std::collections::HashMap;

use anyhow::Result;
use codex_protocol::protocol::McpAuthStatus;
use codex_rmcp_client::OAuthCredentialsStoreMode;
use codex_rmcp_client::OAuthProviderError;
use codex_rmcp_client::determine_streamable_http_auth_status;
use codex_rmcp_client::discover_streamable_http_oauth;
use futures::future::join_all;
use tracing::warn;

use crate::config::types::McpServerConfig;
use crate::config::types::McpServerTransportConfig;

#[derive(Debug, Clone)]
pub struct McpOAuthLoginConfig {
    pub url: String,
    pub http_headers: Option<HashMap<String, String>>,
    pub env_http_headers: Option<HashMap<String, String>>,
    pub discovered_scopes: Option<Vec<String>>,
    pub oauth_authorization_params: Option<HashMap<String, String>>,
    pub oauth_client_metadata_url: Option<String>,
}

#[derive(Debug)]
pub enum McpOAuthLoginSupport {
    Supported(McpOAuthLoginConfig),
    Unsupported,
    Unknown(anyhow::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum McpOAuthScopesSource {
    Explicit,
    Configured,
    Discovered,
    Empty,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedMcpOAuthScopes {
    pub scopes: Vec<String>,
    pub source: McpOAuthScopesSource,
}

fn merge_string_maps(
    base: Option<HashMap<String, String>>,
    overrides: Option<HashMap<String, String>>,
) -> Option<HashMap<String, String>> {
    let mut merged = base.unwrap_or_default();
    merged.extend(overrides.unwrap_or_default());
    (!merged.is_empty()).then_some(merged)
}

fn resolved_oauth_headers(
    config: &McpServerConfig,
) -> (Option<HashMap<String, String>>, Option<HashMap<String, String>>) {
    let McpServerTransportConfig::StreamableHttp {
        http_headers,
        env_http_headers,
        ..
    } = &config.transport
    else {
        return (None, None);
    };

    (
        merge_string_maps(http_headers.clone(), config.oauth_http_headers.clone()),
        merge_string_maps(
            env_http_headers.clone(),
            config.oauth_env_http_headers.clone(),
        ),
    )
}

pub async fn oauth_login_support(config: &McpServerConfig) -> McpOAuthLoginSupport {
    let McpServerTransportConfig::StreamableHttp {
        url,
        bearer_token_env_var,
        ..
    } = &config.transport
    else {
        return McpOAuthLoginSupport::Unsupported;
    };

    if bearer_token_env_var.is_some() {
        return McpOAuthLoginSupport::Unsupported;
    }

    let (http_headers, env_http_headers) = resolved_oauth_headers(config);

    match discover_streamable_http_oauth(url, http_headers.clone(), env_http_headers.clone()).await {
        Ok(Some(discovery)) => McpOAuthLoginSupport::Supported(McpOAuthLoginConfig {
            url: url.clone(),
            http_headers,
            env_http_headers,
            discovered_scopes: discovery.scopes_supported,
            oauth_authorization_params: config.oauth_authorization_params.clone(),
            oauth_client_metadata_url: config.oauth_client_metadata_url.clone(),
        }),
        Ok(None) => McpOAuthLoginSupport::Unsupported,
        Err(err) => McpOAuthLoginSupport::Unknown(err),
    }
}

pub async fn discover_supported_scopes(config: &McpServerConfig) -> Option<Vec<String>> {
    match oauth_login_support(config).await {
        McpOAuthLoginSupport::Supported(config) => config.discovered_scopes,
        McpOAuthLoginSupport::Unsupported | McpOAuthLoginSupport::Unknown(_) => None,
    }
}

pub fn resolve_oauth_scopes(
    explicit_scopes: Option<Vec<String>>,
    configured_scopes: Option<Vec<String>>,
    discovered_scopes: Option<Vec<String>>,
) -> ResolvedMcpOAuthScopes {
    if let Some(scopes) = explicit_scopes {
        return ResolvedMcpOAuthScopes {
            scopes,
            source: McpOAuthScopesSource::Explicit,
        };
    }

    if let Some(scopes) = configured_scopes {
        return ResolvedMcpOAuthScopes {
            scopes,
            source: McpOAuthScopesSource::Configured,
        };
    }

    if let Some(scopes) = discovered_scopes
        && !scopes.is_empty()
    {
        return ResolvedMcpOAuthScopes {
            scopes,
            source: McpOAuthScopesSource::Discovered,
        };
    }

    ResolvedMcpOAuthScopes {
        scopes: Vec::new(),
        source: McpOAuthScopesSource::Empty,
    }
}

pub fn should_retry_without_scopes(scopes: &ResolvedMcpOAuthScopes, error: &anyhow::Error) -> bool {
    scopes.source == McpOAuthScopesSource::Discovered
        && error.downcast_ref::<OAuthProviderError>().is_some()
}

#[derive(Debug, Clone)]
pub struct McpAuthStatusEntry {
    pub config: McpServerConfig,
    pub auth_status: McpAuthStatus,
}

pub async fn compute_auth_statuses<'a, I>(
    servers: I,
    store_mode: OAuthCredentialsStoreMode,
) -> HashMap<String, McpAuthStatusEntry>
where
    I: IntoIterator<Item = (&'a String, &'a McpServerConfig)>,
{
    let futures = servers.into_iter().map(|(name, config)| {
        let name = name.clone();
        let config = config.clone();
        async move {
            let auth_status = match compute_auth_status(&name, &config, store_mode).await {
                Ok(status) => status,
                Err(error) => {
                    warn!("failed to determine auth status for MCP server `{name}`: {error:?}");
                    McpAuthStatus::Unsupported
                }
            };
            let entry = McpAuthStatusEntry {
                config,
                auth_status,
            };
            (name, entry)
        }
    });

    join_all(futures).await.into_iter().collect()
}

async fn compute_auth_status(
    server_name: &str,
    config: &McpServerConfig,
    store_mode: OAuthCredentialsStoreMode,
) -> Result<McpAuthStatus> {
    match &config.transport {
        McpServerTransportConfig::Stdio { .. } => Ok(McpAuthStatus::Unsupported),
        McpServerTransportConfig::StreamableHttp {
            url,
            bearer_token_env_var,
            http_headers,
            env_http_headers,
        } => {
            let (oauth_http_headers, oauth_env_http_headers) = resolved_oauth_headers(config);
            determine_streamable_http_auth_status(
                server_name,
                url,
                bearer_token_env_var.as_deref(),
                http_headers.clone(),
                env_http_headers.clone(),
                oauth_http_headers,
                oauth_env_http_headers,
                store_mode,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use anyhow::anyhow;
    use pretty_assertions::assert_eq;

    use super::McpOAuthScopesSource;
    use super::OAuthProviderError;
    use super::ResolvedMcpOAuthScopes;
    use super::resolve_oauth_scopes;
    use super::should_retry_without_scopes;

    #[test]
    fn resolve_oauth_scopes_prefers_explicit() {
        let resolved = resolve_oauth_scopes(
            Some(vec!["explicit".to_string()]),
            Some(vec!["configured".to_string()]),
            Some(vec!["discovered".to_string()]),
        );

        assert_eq!(
            resolved,
            ResolvedMcpOAuthScopes {
                scopes: vec!["explicit".to_string()],
                source: McpOAuthScopesSource::Explicit,
            }
        );
    }

    #[test]
    fn resolve_oauth_scopes_prefers_configured_over_discovered() {
        let resolved = resolve_oauth_scopes(
            None,
            Some(vec!["configured".to_string()]),
            Some(vec!["discovered".to_string()]),
        );

        assert_eq!(
            resolved,
            ResolvedMcpOAuthScopes {
                scopes: vec!["configured".to_string()],
                source: McpOAuthScopesSource::Configured,
            }
        );
    }

    #[test]
    fn resolve_oauth_scopes_uses_discovered_when_needed() {
        let resolved = resolve_oauth_scopes(None, None, Some(vec!["discovered".to_string()]));

        assert_eq!(
            resolved,
            ResolvedMcpOAuthScopes {
                scopes: vec!["discovered".to_string()],
                source: McpOAuthScopesSource::Discovered,
            }
        );
    }

    #[test]
    fn resolve_oauth_scopes_preserves_explicitly_empty_configured_scopes() {
        let resolved = resolve_oauth_scopes(None, Some(Vec::new()), Some(vec!["ignored".into()]));

        assert_eq!(
            resolved,
            ResolvedMcpOAuthScopes {
                scopes: Vec::new(),
                source: McpOAuthScopesSource::Configured,
            }
        );
    }

    #[test]
    fn resolve_oauth_scopes_falls_back_to_empty() {
        let resolved = resolve_oauth_scopes(None, None, None);

        assert_eq!(
            resolved,
            ResolvedMcpOAuthScopes {
                scopes: Vec::new(),
                source: McpOAuthScopesSource::Empty,
            }
        );
    }

    #[test]
    fn should_retry_without_scopes_only_for_discovered_provider_errors() {
        let discovered = ResolvedMcpOAuthScopes {
            scopes: vec!["scope".to_string()],
            source: McpOAuthScopesSource::Discovered,
        };
        let provider_error = anyhow!(OAuthProviderError::new(
            Some("invalid_scope".to_string()),
            Some("scope rejected".to_string()),
        ));

        assert!(should_retry_without_scopes(&discovered, &provider_error));

        let configured = ResolvedMcpOAuthScopes {
            scopes: vec!["scope".to_string()],
            source: McpOAuthScopesSource::Configured,
        };
        assert!(!should_retry_without_scopes(&configured, &provider_error));
        assert!(!should_retry_without_scopes(
            &discovered,
            &anyhow!("timed out waiting for OAuth callback"),
        ));
    }
}
