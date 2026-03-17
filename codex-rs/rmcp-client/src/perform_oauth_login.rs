use std::collections::HashMap;
use std::string::String;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use anyhow::Result;
use anyhow::anyhow;
use anyhow::bail;
use oauth2::AuthUrl;
use oauth2::AuthorizationCode;
use oauth2::ClientId;
use oauth2::ClientSecret;
use oauth2::CsrfToken;
use oauth2::EmptyExtraTokenFields;
use oauth2::EndpointNotSet;
use oauth2::EndpointSet;
use oauth2::PkceCodeChallenge;
use oauth2::PkceCodeVerifier;
use oauth2::RedirectUrl;
use oauth2::RequestTokenError;
use oauth2::RevocationErrorResponseType;
use oauth2::Scope;
use oauth2::StandardErrorResponse;
use oauth2::StandardRevocableToken;
use oauth2::StandardTokenIntrospectionResponse;
use oauth2::StandardTokenResponse;
use oauth2::TokenUrl;
use oauth2::basic::BasicClient;
use oauth2::basic::BasicErrorResponseType;
use oauth2::basic::BasicTokenType;
use reqwest::ClientBuilder;
use reqwest::Url;
use rmcp::transport::auth::AuthorizationManager;
use rmcp::transport::auth::AuthorizationMetadata;
use rmcp::transport::auth::OAuthClientConfig;
use tiny_http::Response;
use tiny_http::Server;
use tokio::sync::oneshot;
use tokio::time::timeout;
use urlencoding::decode;

use crate::OAuthCredentialsStoreMode;
use crate::StoredOAuthTokens;
use crate::WrappedOAuthTokenResponse;
use crate::oauth::compute_expires_at_millis;
use crate::save_oauth_tokens;
use crate::utils::apply_default_headers;
use crate::utils::build_default_headers;

type OAuthErrorResponse = StandardErrorResponse<BasicErrorResponseType>;
type OAuthTokenExchangeResponse = StandardTokenResponse<EmptyExtraTokenFields, BasicTokenType>;
type OAuthTokenIntrospection =
    StandardTokenIntrospectionResponse<EmptyExtraTokenFields, BasicTokenType>;
type OAuthRevocationError = StandardErrorResponse<RevocationErrorResponseType>;
type OAuthClient = oauth2::Client<
    OAuthErrorResponse,
    OAuthTokenExchangeResponse,
    OAuthTokenIntrospection,
    StandardRevocableToken,
    OAuthRevocationError,
    EndpointSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointNotSet,
    EndpointSet,
>;

struct OauthHeaders {
    http_headers: Option<HashMap<String, String>>,
    env_http_headers: Option<HashMap<String, String>>,
}

struct CallbackServerGuard {
    server: Arc<Server>,
}

impl Drop for CallbackServerGuard {
    fn drop(&mut self) {
        self.server.unblock();
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OAuthProviderError {
    error: Option<String>,
    error_description: Option<String>,
}

impl OAuthProviderError {
    pub fn new(error: Option<String>, error_description: Option<String>) -> Self {
        Self {
            error,
            error_description,
        }
    }
}

impl std::fmt::Display for OAuthProviderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match (self.error.as_deref(), self.error_description.as_deref()) {
            (Some(error), Some(error_description)) => {
                write!(f, "OAuth provider returned `{error}`: {error_description}")
            }
            (Some(error), None) => write!(f, "OAuth provider returned `{error}`"),
            (None, Some(error_description)) => write!(f, "OAuth error: {error_description}"),
            (None, None) => write!(f, "OAuth provider returned an error"),
        }
    }
}

impl std::error::Error for OAuthProviderError {}

#[allow(clippy::too_many_arguments)]
pub async fn perform_oauth_login(
    server_name: &str,
    server_url: &str,
    store_mode: OAuthCredentialsStoreMode,
    http_headers: Option<HashMap<String, String>>,
    env_http_headers: Option<HashMap<String, String>>,
    scopes: &[String],
    oauth_resource: Option<&str>,
    oauth_authorization_params: Option<HashMap<String, String>>,
    oauth_client_metadata_url: Option<&str>,
    callback_port: Option<u16>,
    callback_url: Option<&str>,
) -> Result<()> {
    let headers = OauthHeaders {
        http_headers,
        env_http_headers,
    };
    OauthLoginFlow::new(
        server_name,
        server_url,
        store_mode,
        headers,
        scopes,
        oauth_resource,
        oauth_authorization_params,
        oauth_client_metadata_url,
        true,
        callback_port,
        callback_url,
        /*timeout_secs*/ None,
    )
    .await?
    .finish()
    .await
}

#[allow(clippy::too_many_arguments)]
pub async fn perform_oauth_login_return_url(
    server_name: &str,
    server_url: &str,
    store_mode: OAuthCredentialsStoreMode,
    http_headers: Option<HashMap<String, String>>,
    env_http_headers: Option<HashMap<String, String>>,
    scopes: &[String],
    oauth_resource: Option<&str>,
    oauth_authorization_params: Option<HashMap<String, String>>,
    oauth_client_metadata_url: Option<&str>,
    timeout_secs: Option<i64>,
    callback_port: Option<u16>,
    callback_url: Option<&str>,
) -> Result<OauthLoginHandle> {
    let headers = OauthHeaders {
        http_headers,
        env_http_headers,
    };
    let flow = OauthLoginFlow::new(
        server_name,
        server_url,
        store_mode,
        headers,
        scopes,
        oauth_resource,
        oauth_authorization_params,
        oauth_client_metadata_url,
        false,
        callback_port,
        callback_url,
        timeout_secs,
    )
    .await?;

    let authorization_url = flow.authorization_url();
    let completion = flow.spawn();

    Ok(OauthLoginHandle::new(authorization_url, completion))
}

fn spawn_callback_server(
    server: Arc<Server>,
    tx: oneshot::Sender<CallbackResult>,
    expected_callback_path: String,
) {
    tokio::task::spawn_blocking(move || {
        while let Ok(request) = server.recv() {
            let path = request.url().to_string();
            match parse_oauth_callback(&path, &expected_callback_path) {
                CallbackOutcome::Success(OauthCallbackResult { code, state }) => {
                    let response = Response::from_string(
                        "Authentication complete. You may close this window.",
                    );
                    if let Err(err) = request.respond(response) {
                        eprintln!("Failed to respond to OAuth callback: {err}");
                    }
                    if let Err(err) =
                        tx.send(CallbackResult::Success(OauthCallbackResult { code, state }))
                    {
                        eprintln!("Failed to send OAuth callback: {err:?}");
                    }
                    break;
                }
                CallbackOutcome::Error(error) => {
                    let response = Response::from_string(error.to_string()).with_status_code(400);
                    if let Err(err) = request.respond(response) {
                        eprintln!("Failed to respond to OAuth callback: {err}");
                    }
                    if let Err(err) = tx.send(CallbackResult::Error(error)) {
                        eprintln!("Failed to send OAuth callback error: {err:?}");
                    }
                    break;
                }
                CallbackOutcome::Invalid => {
                    let response =
                        Response::from_string("Invalid OAuth callback").with_status_code(400);
                    if let Err(err) = request.respond(response) {
                        eprintln!("Failed to respond to OAuth callback: {err}");
                    }
                }
            }
        }
    });
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct OauthCallbackResult {
    code: String,
    state: String,
}

#[derive(Debug)]
enum CallbackResult {
    Success(OauthCallbackResult),
    Error(OAuthProviderError),
}

#[derive(Debug, PartialEq, Eq)]
enum CallbackOutcome {
    Success(OauthCallbackResult),
    Error(OAuthProviderError),
    Invalid,
}

fn parse_oauth_callback(path: &str, expected_callback_path: &str) -> CallbackOutcome {
    let Some((route, query)) = path.split_once('?') else {
        return CallbackOutcome::Invalid;
    };
    if route != expected_callback_path {
        return CallbackOutcome::Invalid;
    }

    let mut code = None;
    let mut state = None;
    let mut error = None;
    let mut error_description = None;

    for pair in query.split('&') {
        let Some((key, value)) = pair.split_once('=') else {
            continue;
        };
        let Ok(decoded) = decode(value) else {
            continue;
        };
        let decoded = decoded.into_owned();
        match key {
            "code" => code = Some(decoded),
            "state" => state = Some(decoded),
            "error" => error = Some(decoded),
            "error_description" => error_description = Some(decoded),
            _ => {}
        }
    }

    if let (Some(code), Some(state)) = (code, state) {
        return CallbackOutcome::Success(OauthCallbackResult { code, state });
    }

    if error.is_some() || error_description.is_some() {
        return CallbackOutcome::Error(OAuthProviderError::new(error, error_description));
    }

    CallbackOutcome::Invalid
}

pub struct OauthLoginHandle {
    authorization_url: String,
    completion: oneshot::Receiver<Result<()>>,
}

impl OauthLoginHandle {
    fn new(authorization_url: String, completion: oneshot::Receiver<Result<()>>) -> Self {
        Self {
            authorization_url,
            completion,
        }
    }

    pub fn authorization_url(&self) -> &str {
        &self.authorization_url
    }

    pub fn into_parts(self) -> (String, oneshot::Receiver<Result<()>>) {
        (self.authorization_url, self.completion)
    }

    pub async fn wait(self) -> Result<()> {
        self.completion
            .await
            .map_err(|err| anyhow!("OAuth login task was cancelled: {err}"))?
    }
}

struct OauthLoginFlow {
    auth_url: String,
    oauth_metadata: AuthorizationMetadata,
    oauth_client_config: OAuthClientConfig,
    http_client: reqwest::Client,
    csrf_token: String,
    pkce_verifier: PkceCodeVerifier,
    rx: oneshot::Receiver<CallbackResult>,
    guard: CallbackServerGuard,
    server_name: String,
    server_url: String,
    store_mode: OAuthCredentialsStoreMode,
    launch_browser: bool,
    timeout: Duration,
}

fn resolve_callback_port(callback_port: Option<u16>) -> Result<Option<u16>> {
    if let Some(config_port) = callback_port {
        if config_port == 0 {
            bail!(
                "invalid MCP OAuth callback port `{config_port}`: port must be between 1 and 65535"
            );
        }
        return Ok(Some(config_port));
    }

    Ok(None)
}

fn local_redirect_uri(server: &Server) -> Result<String> {
    match server.server_addr() {
        tiny_http::ListenAddr::IP(std::net::SocketAddr::V4(addr)) => {
            let ip = addr.ip();
            let port = addr.port();
            Ok(format!("http://{ip}:{port}/callback"))
        }
        tiny_http::ListenAddr::IP(std::net::SocketAddr::V6(addr)) => {
            let ip = addr.ip();
            let port = addr.port();
            Ok(format!("http://[{ip}]:{port}/callback"))
        }
        #[cfg(not(target_os = "windows"))]
        _ => Err(anyhow!("unable to determine callback address")),
    }
}

fn resolve_redirect_uri(server: &Server, callback_url: Option<&str>) -> Result<String> {
    let Some(callback_url) = callback_url else {
        return local_redirect_uri(server);
    };
    Url::parse(callback_url)
        .with_context(|| format!("invalid MCP OAuth callback URL `{callback_url}`"))?;
    Ok(callback_url.to_string())
}

fn callback_path_from_redirect_uri(redirect_uri: &str) -> Result<String> {
    let parsed = Url::parse(redirect_uri)
        .with_context(|| format!("invalid redirect URI `{redirect_uri}`"))?;
    Ok(parsed.path().to_string())
}

fn callback_bind_host(callback_url: Option<&str>) -> &'static str {
    let Some(callback_url) = callback_url else {
        return "127.0.0.1";
    };

    let Ok(parsed) = Url::parse(callback_url) else {
        return "127.0.0.1";
    };

    match parsed.host_str() {
        Some("localhost" | "127.0.0.1" | "::1") | None => "127.0.0.1",
        Some(_) => "0.0.0.0",
    }
}

fn supports_url_based_client_id(metadata: &AuthorizationMetadata) -> bool {
    metadata
        .additional_fields
        .get("client_id_metadata_document_supported")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

fn validate_client_metadata_url(client_metadata_url: &str) -> Result<()> {
    let parsed = Url::parse(client_metadata_url)
        .with_context(|| format!("invalid OAuth client metadata URL `{client_metadata_url}`"))?;
    let is_valid =
        parsed.scheme() == "https" && parsed.host_str().is_some() && parsed.path() != "/";
    if is_valid {
        Ok(())
    } else {
        bail!(
            "OAuth client metadata URL must use https and include a non-root path: {client_metadata_url}"
        );
    }
}

async fn resolve_oauth_client_config(
    auth_manager: &mut AuthorizationManager,
    metadata: &AuthorizationMetadata,
    redirect_uri: &str,
    scopes: &[String],
    oauth_client_metadata_url: Option<&str>,
) -> Result<OAuthClientConfig> {
    if supports_url_based_client_id(metadata)
        && let Some(client_metadata_url) = oauth_client_metadata_url
    {
        validate_client_metadata_url(client_metadata_url)?;
        return Ok(OAuthClientConfig {
            client_id: client_metadata_url.to_string(),
            client_secret: None,
            scopes: scopes.to_vec(),
            redirect_uri: redirect_uri.to_string(),
        });
    }

    let mut config = auth_manager.register_client("Codex", redirect_uri).await?;
    config.scopes = scopes.to_vec();
    Ok(config)
}

fn build_oauth_client(
    metadata: &AuthorizationMetadata,
    config: &OAuthClientConfig,
) -> Result<OAuthClient> {
    let auth_url = AuthUrl::new(metadata.authorization_endpoint.clone())
        .with_context(|| "invalid OAuth authorization endpoint")?;
    let token_url = TokenUrl::new(metadata.token_endpoint.clone())
        .with_context(|| "invalid OAuth token endpoint")?;
    let redirect_url = RedirectUrl::new(config.redirect_uri.clone())
        .with_context(|| "invalid OAuth redirect URI")?;

    let mut client = BasicClient::new(ClientId::new(config.client_id.clone()))
        .set_auth_uri(auth_url)
        .set_token_uri(token_url)
        .set_redirect_uri(redirect_url);

    if let Some(secret) = &config.client_secret {
        client = client.set_client_secret(ClientSecret::new(secret.clone()));
    }

    Ok(client)
}

fn build_authorization_url(
    oauth_client: &OAuthClient,
    scopes: &[String],
) -> (String, String, PkceCodeVerifier) {
    let (pkce_challenge, pkce_verifier) = PkceCodeChallenge::new_random_sha256();
    let mut request = oauth_client
        .authorize_url(CsrfToken::new_random)
        .set_pkce_challenge(pkce_challenge);

    for scope in scopes {
        request = request.add_scope(Scope::new(scope.clone()));
    }

    let (auth_url, csrf_token) = request.url();
    (
        auth_url.to_string(),
        csrf_token.secret().to_string(),
        pkce_verifier,
    )
}

async fn exchange_code_for_token(
    oauth_metadata: &AuthorizationMetadata,
    oauth_client_config: &OAuthClientConfig,
    http_client: &reqwest::Client,
    code: String,
    pkce_verifier: PkceCodeVerifier,
) -> Result<OAuthTokenExchangeResponse> {
    let oauth_client = build_oauth_client(oauth_metadata, oauth_client_config)?;
    let token_result = oauth_client
        .exchange_code(AuthorizationCode::new(code))
        .set_pkce_verifier(pkce_verifier)
        .request_async(http_client)
        .await;

    match token_result {
        Ok(token) => Ok(token),
        Err(RequestTokenError::Parse(_, body)) => {
            serde_json::from_slice::<OAuthTokenExchangeResponse>(&body)
                .context("failed to parse OAuth token response")
        }
        Err(err) => Err(anyhow!("failed to exchange OAuth code for token: {err}")),
    }
}

impl OauthLoginFlow {
    #[allow(clippy::too_many_arguments)]
    async fn new(
        server_name: &str,
        server_url: &str,
        store_mode: OAuthCredentialsStoreMode,
        headers: OauthHeaders,
        scopes: &[String],
        oauth_resource: Option<&str>,
        oauth_authorization_params: Option<HashMap<String, String>>,
        oauth_client_metadata_url: Option<&str>,
        launch_browser: bool,
        callback_port: Option<u16>,
        callback_url: Option<&str>,
        timeout_secs: Option<i64>,
    ) -> Result<Self> {
        const DEFAULT_OAUTH_TIMEOUT_SECS: i64 = 300;

        let bind_host = callback_bind_host(callback_url);
        let callback_port = resolve_callback_port(callback_port)?;
        let bind_addr = match callback_port {
            Some(port) => format!("{bind_host}:{port}"),
            None => format!("{bind_host}:0"),
        };

        let server = Arc::new(Server::http(&bind_addr).map_err(|err| anyhow!(err))?);
        let guard = CallbackServerGuard {
            server: Arc::clone(&server),
        };

        let redirect_uri = resolve_redirect_uri(&server, callback_url)?;
        let callback_path = callback_path_from_redirect_uri(&redirect_uri)?;

        let (tx, rx) = oneshot::channel();
        spawn_callback_server(server, tx, callback_path);

        let OauthHeaders {
            http_headers,
            env_http_headers,
        } = headers;
        let default_headers = build_default_headers(http_headers, env_http_headers)?;
        let http_client = apply_default_headers(ClientBuilder::new(), &default_headers).build()?;

        let mut auth_manager = AuthorizationManager::new(server_url).await?;
        auth_manager.with_client(http_client.clone())?;
        let oauth_metadata = auth_manager.discover_metadata().await?;
        auth_manager.set_metadata(oauth_metadata.clone());
        let oauth_client_config = resolve_oauth_client_config(
            &mut auth_manager,
            &oauth_metadata,
            &redirect_uri,
            scopes,
            oauth_client_metadata_url,
        )
        .await?;
        let oauth_client = build_oauth_client(&oauth_metadata, &oauth_client_config)?;
        let (authorization_url, csrf_token, pkce_verifier) =
            build_authorization_url(&oauth_client, scopes);
        let auth_url = append_query_params(
            &authorization_url,
            oauth_resource,
            oauth_authorization_params.as_ref(),
        );
        let timeout_secs = timeout_secs.unwrap_or(DEFAULT_OAUTH_TIMEOUT_SECS).max(1);
        let timeout = Duration::from_secs(timeout_secs as u64);

        Ok(Self {
            auth_url,
            oauth_metadata,
            oauth_client_config,
            http_client,
            csrf_token,
            pkce_verifier,
            rx,
            guard,
            server_name: server_name.to_string(),
            server_url: server_url.to_string(),
            store_mode,
            launch_browser,
            timeout,
        })
    }

    fn authorization_url(&self) -> String {
        self.auth_url.clone()
    }

    async fn finish(mut self) -> Result<()> {
        if self.launch_browser {
            let server_name = &self.server_name;
            let auth_url = &self.auth_url;
            println!(
                "Authorize `{server_name}` by opening this URL in your browser:\n{auth_url}\n"
            );

            if webbrowser::open(auth_url).is_err() {
                println!("(Browser launch failed; please copy the URL above manually.)");
            }
        }

        let result = async {
            let callback = timeout(self.timeout, &mut self.rx)
                .await
                .context("timed out waiting for OAuth callback")?
                .context("OAuth callback was cancelled")?;
            let OauthCallbackResult {
                code,
                state: csrf_state,
            } = match callback {
                CallbackResult::Success(callback) => callback,
                CallbackResult::Error(error) => return Err(anyhow!(error)),
            };

            if csrf_state != self.csrf_token {
                bail!("OAuth state mismatch in callback");
            }

            let credentials = exchange_code_for_token(
                &self.oauth_metadata,
                &self.oauth_client_config,
                &self.http_client,
                code,
                self.pkce_verifier,
            )
            .await?;

            let expires_at = compute_expires_at_millis(&credentials);
            let stored = StoredOAuthTokens {
                server_name: self.server_name.clone(),
                url: self.server_url.clone(),
                client_id: self.oauth_client_config.client_id.clone(),
                token_response: WrappedOAuthTokenResponse(credentials),
                expires_at,
            };
            save_oauth_tokens(&self.server_name, &stored, self.store_mode)?;

            Ok(())
        }
        .await;

        drop(self.guard);
        result
    }

    fn spawn(self) -> oneshot::Receiver<Result<()>> {
        let server_name_for_logging = self.server_name.clone();
        let (tx, rx) = oneshot::channel();

        tokio::spawn(async move {
            let result = self.finish().await;

            if let Err(err) = &result {
                eprintln!(
                    "Failed to complete OAuth login for '{server_name_for_logging}': {err:#}"
                );
            }

            let _ = tx.send(result);
        });

        rx
    }
}

fn append_query_params(
    url: &str,
    oauth_resource: Option<&str>,
    authorization_params: Option<&HashMap<String, String>>,
) -> String {
    if let Ok(mut parsed) = Url::parse(url) {
        {
            let mut pairs = parsed.query_pairs_mut();
            if let Some(resource) = oauth_resource
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                pairs.append_pair("resource", resource);
            }
            if let Some(params) = authorization_params {
                for (key, value) in params {
                    let trimmed_key = key.trim();
                    let trimmed_value = value.trim();
                    if trimmed_key.is_empty() || trimmed_value.is_empty() {
                        continue;
                    }
                    pairs.append_pair(trimmed_key, trimmed_value);
                }
            }
        }
        return parsed.to_string();
    }

    let mut fragments = Vec::new();
    if let Some(resource) = oauth_resource
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        fragments.push(format!("resource={}", urlencoding::encode(resource)));
    }
    if let Some(params) = authorization_params {
        for (key, value) in params {
            let trimmed_key = key.trim();
            let trimmed_value = value.trim();
            if trimmed_key.is_empty() || trimmed_value.is_empty() {
                continue;
            }
            fragments.push(format!(
                "{}={}",
                urlencoding::encode(trimmed_key),
                urlencoding::encode(trimmed_value),
            ));
        }
    }
    if fragments.is_empty() {
        return url.to_string();
    }
    let separator = if url.contains('?') { "&" } else { "?" };
    format!("{url}{separator}{}", fragments.join("&"))
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::ffi::OsString;
    use std::sync::Arc;
    use std::sync::Mutex;
    use std::sync::OnceLock;

    use axum::Json;
    use axum::Router;
    use axum::body::Bytes;
    use axum::extract::Query;
    use axum::extract::State;
    use axum::http::HeaderMap;
    use axum::http::StatusCode;
    use axum::response::Redirect;
    use axum::routing::get;
    use axum::routing::post;
    use oauth2::TokenResponse;
    use pretty_assertions::assert_eq;
    use serial_test::serial;
    use tempfile::TempDir;
    use tokio::task::JoinHandle;

    use crate::OAuthCredentialsStoreMode;
    use crate::oauth::load_oauth_tokens;

    use super::CallbackOutcome;
    use super::OAuthProviderError;
    use super::append_query_params;
    use super::callback_path_from_redirect_uri;
    use super::parse_oauth_callback;
    use super::perform_oauth_login_return_url;

    #[derive(Debug, Clone)]
    struct LoggedRequest {
        method: String,
        path: String,
        query: HashMap<String, String>,
        headers: HashMap<String, String>,
        body: String,
    }

    #[derive(Clone)]
    struct TestOAuthServerState {
        base_url: String,
        supports_url_based_client_id: bool,
        requests: Arc<Mutex<Vec<LoggedRequest>>>,
    }

    struct TestOAuthServer {
        url: String,
        requests: Arc<Mutex<Vec<LoggedRequest>>>,
        handle: JoinHandle<()>,
    }

    impl TestOAuthServer {
        fn requests(&self) -> Vec<LoggedRequest> {
            self.requests
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone()
        }
    }

    impl Drop for TestOAuthServer {
        fn drop(&mut self) {
            self.handle.abort();
        }
    }

    fn capture_headers(headers: &HeaderMap) -> HashMap<String, String> {
        headers
            .iter()
            .map(|(name, value)| {
                (
                    name.as_str().to_ascii_lowercase(),
                    value.to_str().unwrap_or("<non-utf8>").to_string(),
                )
            })
            .collect()
    }

    fn record_request(
        state: &TestOAuthServerState,
        method: &str,
        path: &str,
        query: HashMap<String, String>,
        headers: &HeaderMap,
        body: String,
    ) {
        let mut requests = state
            .requests
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        requests.push(LoggedRequest {
            method: method.to_string(),
            path: path.to_string(),
            query,
            headers: capture_headers(headers),
            body,
        });
    }

    async fn oauth_metadata_handler(
        State(state): State<TestOAuthServerState>,
        headers: HeaderMap,
    ) -> Json<serde_json::Value> {
        record_request(
            &state,
            "GET",
            "/.well-known/oauth-authorization-server/mcp",
            HashMap::new(),
            &headers,
            String::new(),
        );
        Json(serde_json::json!({
            "authorization_endpoint": format!("{}/oauth/authorize", state.base_url),
            "token_endpoint": format!("{}/oauth/token", state.base_url),
            "registration_endpoint": format!("{}/oauth/register", state.base_url),
            "response_types_supported": ["code"],
            "scopes_supported": ["profile", "email"],
            "client_id_metadata_document_supported": state.supports_url_based_client_id,
        }))
    }

    async fn oauth_authorize_handler(
        State(state): State<TestOAuthServerState>,
        Query(query): Query<HashMap<String, String>>,
        headers: HeaderMap,
    ) -> Redirect {
        record_request(
            &state,
            "GET",
            "/oauth/authorize",
            query.clone(),
            &headers,
            String::new(),
        );
        let redirect_uri = query
            .get("redirect_uri")
            .expect("authorization request should include redirect_uri");
        let state_param = query
            .get("state")
            .expect("authorization request should include state");
        let separator = if redirect_uri.contains('?') { "&" } else { "?" };
        Redirect::temporary(&format!(
            "{redirect_uri}{separator}code=test-authorization-code&state={}",
            urlencoding::encode(state_param),
        ))
    }

    async fn oauth_register_handler(
        State(state): State<TestOAuthServerState>,
        headers: HeaderMap,
        body: Bytes,
    ) -> (StatusCode, Json<serde_json::Value>) {
        let body = String::from_utf8(body.to_vec()).expect("registration body should be utf-8");
        record_request(
            &state,
            "POST",
            "/oauth/register",
            HashMap::new(),
            &headers,
            body,
        );
        (
            StatusCode::CREATED,
            Json(serde_json::json!({
                "client_id": "dynamic-client-id",
                "client_secret": "",
                "client_name": "Codex",
                "redirect_uris": ["http://127.0.0.1/callback"],
            })),
        )
    }

    async fn oauth_token_handler(
        State(state): State<TestOAuthServerState>,
        headers: HeaderMap,
        body: Bytes,
    ) -> Json<serde_json::Value> {
        let body = String::from_utf8(body.to_vec()).expect("token body should be utf-8");
        record_request(
            &state,
            "POST",
            "/oauth/token",
            HashMap::new(),
            &headers,
            body,
        );
        Json(serde_json::json!({
            "access_token": "test-access-token",
            "token_type": "Bearer",
            "refresh_token": "test-refresh-token",
            "expires_in": 3600,
            "scope": "profile email",
        }))
    }

    async fn spawn_test_oauth_server(supports_url_based_client_id: bool) -> TestOAuthServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("listener should bind");
        let address = listener.local_addr().expect("listener should have address");
        let base_url = format!("http://{address}");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let state = TestOAuthServerState {
            base_url: base_url.clone(),
            supports_url_based_client_id,
            requests: Arc::clone(&requests),
        };
        let app = Router::new()
            .route(
                "/.well-known/oauth-authorization-server/mcp",
                get(oauth_metadata_handler),
            )
            .route("/oauth/authorize", get(oauth_authorize_handler))
            .route("/oauth/register", post(oauth_register_handler))
            .route("/oauth/token", post(oauth_token_handler))
            .with_state(state);
        let handle = tokio::spawn(async move {
            axum::serve(listener, app).await.expect("server should run");
        });

        TestOAuthServer {
            url: format!("{base_url}/mcp"),
            requests,
            handle,
        }
    }

    struct EnvVarGuard {
        key: String,
        original: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let original = std::env::var_os(key);
            unsafe {
                std::env::set_var(key, value);
            }
            Self {
                key: key.to_string(),
                original,
            }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(value) = &self.original {
                unsafe {
                    std::env::set_var(&self.key, value);
                }
            } else {
                unsafe {
                    std::env::remove_var(&self.key);
                }
            }
        }
    }

    struct TempCodexHome {
        _guard: std::sync::MutexGuard<'static, ()>,
        _dir: TempDir,
        _env: EnvVarGuard,
    }

    impl TempCodexHome {
        fn new() -> Self {
            static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
            let guard = LOCK
                .get_or_init(Mutex::default)
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let dir = tempfile::tempdir().expect("create CODEX_HOME temp dir");
            let env = EnvVarGuard::set("CODEX_HOME", dir.path().as_os_str());
            Self {
                _guard: guard,
                _dir: dir,
                _env: env,
            }
        }
    }

    fn query_params(url: &str) -> HashMap<String, String> {
        reqwest::Url::parse(url)
            .expect("URL should parse")
            .query_pairs()
            .into_owned()
            .collect()
    }

    fn find_request<'a>(
        requests: &'a [LoggedRequest],
        method: &str,
        path: &str,
    ) -> &'a LoggedRequest {
        requests
            .iter()
            .find(|request| request.method == method && request.path == path)
            .unwrap_or_else(|| panic!("missing {method} {path} request"))
    }

    fn assert_header(request: &LoggedRequest, header_name: &str, expected: &str) {
        assert_eq!(
            request.headers.get(&header_name.to_ascii_lowercase()),
            Some(&expected.to_string())
        );
    }

    #[test]
    fn parse_oauth_callback_accepts_default_path() {
        let parsed = parse_oauth_callback("/callback?code=abc&state=xyz", "/callback");
        assert!(matches!(parsed, CallbackOutcome::Success(_)));
    }

    #[test]
    fn parse_oauth_callback_accepts_custom_path() {
        let parsed = parse_oauth_callback("/oauth/callback?code=abc&state=xyz", "/oauth/callback");
        assert!(matches!(parsed, CallbackOutcome::Success(_)));
    }

    #[test]
    fn parse_oauth_callback_rejects_wrong_path() {
        let parsed = parse_oauth_callback("/callback?code=abc&state=xyz", "/oauth/callback");
        assert!(matches!(parsed, CallbackOutcome::Invalid));
    }

    #[test]
    fn parse_oauth_callback_returns_provider_error() {
        let parsed = parse_oauth_callback(
            "/callback?error=invalid_scope&error_description=scope%20rejected",
            "/callback",
        );

        assert_eq!(
            parsed,
            CallbackOutcome::Error(OAuthProviderError::new(
                Some("invalid_scope".to_string()),
                Some("scope rejected".to_string()),
            ))
        );
    }

    #[test]
    fn callback_path_comes_from_redirect_uri() {
        let path = callback_path_from_redirect_uri("https://example.com/oauth/callback")
            .expect("redirect URI should parse");
        assert_eq!(path, "/oauth/callback");
    }

    #[test]
    fn append_query_params_adds_resource_to_absolute_url() {
        let url = append_query_params(
            "https://example.com/authorize?scope=read",
            Some("https://api.example.com"),
            None,
        );

        assert_eq!(
            url,
            "https://example.com/authorize?scope=read&resource=https%3A%2F%2Fapi.example.com"
        );
    }

    #[test]
    fn append_query_params_ignores_empty_values() {
        let url = append_query_params(
            "https://example.com/authorize?scope=read",
            Some("   "),
            None,
        );

        assert_eq!(url, "https://example.com/authorize?scope=read");
    }

    #[test]
    fn append_query_params_appends_custom_authorization_params() {
        let url = append_query_params(
            "https://example.com/authorize?scope=read",
            Some("https://api.example.com"),
            Some(&HashMap::from([
                (
                    "audience".to_string(),
                    "https://tenant.example.com".to_string(),
                ),
                ("prompt".to_string(), "consent".to_string()),
            ])),
        );

        let parsed = reqwest::Url::parse(&url).expect("URL should parse");
        let params = parsed.query_pairs().into_owned().collect::<HashMap<_, _>>();
        assert_eq!(
            params.get("resource"),
            Some(&"https://api.example.com".to_string())
        );
        assert_eq!(
            params.get("audience"),
            Some(&"https://tenant.example.com".to_string()),
        );
        assert_eq!(params.get("prompt"), Some(&"consent".to_string()));
    }

    #[test]
    fn append_query_params_handles_unparseable_url() {
        let url = append_query_params(
            "not a url",
            Some("api/resource"),
            Some(&HashMap::from([(
                "prompt".to_string(),
                "consent".to_string(),
            )])),
        );

        assert_eq!(url, "not a url?resource=api%2Fresource&prompt=consent");
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial(perform_oauth_login_flow)]
    async fn perform_oauth_login_dynamic_registration_propagates_headers_and_persists_tokens() {
        let _codex_home = TempCodexHome::new();
        let _auth_env = EnvVarGuard::set(
            "CODEX_RMCP_CLIENT_TEST_AUTHORIZATION",
            "Bearer env-oauth-token",
        );
        let _extra_env = EnvVarGuard::set("CODEX_RMCP_CLIENT_TEST_TRACE", "trace-from-env");
        let server = spawn_test_oauth_server(false).await;

        let handle = perform_oauth_login_return_url(
            "server",
            &server.url,
            OAuthCredentialsStoreMode::File,
            Some(HashMap::from([(
                "X-Direct-Header".to_string(),
                "direct-value".to_string(),
            )])),
            Some(HashMap::from([
                (
                    "Authorization".to_string(),
                    "CODEX_RMCP_CLIENT_TEST_AUTHORIZATION".to_string(),
                ),
                (
                    "X-Trace-Header".to_string(),
                    "CODEX_RMCP_CLIENT_TEST_TRACE".to_string(),
                ),
            ])),
            &["profile".to_string(), "email".to_string()],
            Some("https://api.example.com"),
            Some(HashMap::from([
                (
                    "audience".to_string(),
                    "https://tenant.example.com".to_string(),
                ),
                ("prompt".to_string(), "consent".to_string()),
            ])),
            None,
            Some(30),
            None,
            None,
        )
        .await
        .expect("login flow should start");

        let authorization_url = handle.authorization_url().to_string();
        let auth_query = query_params(&authorization_url);
        assert_eq!(
            auth_query.get("resource"),
            Some(&"https://api.example.com".to_string())
        );
        assert_eq!(
            auth_query.get("audience"),
            Some(&"https://tenant.example.com".to_string())
        );
        assert_eq!(auth_query.get("prompt"), Some(&"consent".to_string()));
        assert_eq!(
            auth_query.get("client_id"),
            Some(&"dynamic-client-id".to_string())
        );

        let response = reqwest::get(authorization_url)
            .await
            .expect("authorization request should succeed");
        assert_eq!(response.status(), StatusCode::OK);
        let body = response
            .text()
            .await
            .expect("callback response body should be readable");
        assert!(body.contains("Authentication complete"));

        handle
            .wait()
            .await
            .expect("oauth flow should complete successfully");

        let stored = load_oauth_tokens("server", &server.url, OAuthCredentialsStoreMode::File)
            .expect("stored tokens should load")
            .expect("stored tokens should exist");
        assert_eq!(stored.client_id, "dynamic-client-id");
        assert_eq!(
            stored.token_response.0.access_token().secret(),
            "test-access-token"
        );
        assert_eq!(
            stored
                .token_response
                .0
                .refresh_token()
                .expect("refresh token should be present")
                .secret(),
            "test-refresh-token"
        );
        assert_eq!(
            stored
                .token_response
                .0
                .scopes()
                .expect("scopes should be present")
                .iter()
                .map(|scope| scope.to_string())
                .collect::<Vec<_>>(),
            vec!["profile".to_string(), "email".to_string()]
        );

        let requests = server.requests();
        let discovery = find_request(
            &requests,
            "GET",
            "/.well-known/oauth-authorization-server/mcp",
        );
        let register = find_request(&requests, "POST", "/oauth/register");
        let authorize = find_request(&requests, "GET", "/oauth/authorize");
        let token = find_request(&requests, "POST", "/oauth/token");

        for request in [discovery, register, token] {
            assert_header(request, "x-direct-header", "direct-value");
            assert_header(request, "authorization", "Bearer env-oauth-token");
            assert_header(request, "x-trace-header", "trace-from-env");
        }

        assert!(register.body.contains("\"client_name\":\"Codex\""));
        assert!(token.body.contains("grant_type=authorization_code"));
        assert!(token.body.contains("code=test-authorization-code"));
        assert!(token.body.contains("client_id=dynamic-client-id"));
        assert_eq!(
            authorize.query.get("audience"),
            Some(&"https://tenant.example.com".to_string())
        );
        assert_eq!(
            authorize.query.get("resource"),
            Some(&"https://api.example.com".to_string())
        );
        assert_eq!(authorize.query.get("prompt"), Some(&"consent".to_string()));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    #[serial(perform_oauth_login_flow)]
    async fn perform_oauth_login_uses_client_metadata_url_without_dynamic_registration() {
        let _codex_home = TempCodexHome::new();
        let server = spawn_test_oauth_server(true).await;

        let handle = perform_oauth_login_return_url(
            "server",
            &server.url,
            OAuthCredentialsStoreMode::File,
            Some(HashMap::from([(
                "X-OAuth-Header".to_string(),
                "metadata-flow".to_string(),
            )])),
            None,
            &["profile".to_string()],
            Some("https://api.example.com"),
            Some(HashMap::from([(
                "login_hint".to_string(),
                "alice@example.com".to_string(),
            )])),
            Some("https://client.example.com/metadata.json"),
            Some(30),
            None,
            None,
        )
        .await
        .expect("login flow should start");

        let authorization_url = handle.authorization_url().to_string();
        let auth_query = query_params(&authorization_url);
        assert_eq!(
            auth_query.get("client_id"),
            Some(&"https://client.example.com/metadata.json".to_string())
        );
        assert_eq!(
            auth_query.get("login_hint"),
            Some(&"alice@example.com".to_string())
        );
        assert_eq!(
            auth_query.get("resource"),
            Some(&"https://api.example.com".to_string())
        );

        reqwest::get(authorization_url)
            .await
            .expect("authorization request should succeed");

        handle
            .wait()
            .await
            .expect("oauth flow should complete successfully");

        let stored = load_oauth_tokens("server", &server.url, OAuthCredentialsStoreMode::File)
            .expect("stored tokens should load")
            .expect("stored tokens should exist");
        assert_eq!(stored.client_id, "https://client.example.com/metadata.json");

        let requests = server.requests();
        let discovery = find_request(
            &requests,
            "GET",
            "/.well-known/oauth-authorization-server/mcp",
        );
        let token = find_request(&requests, "POST", "/oauth/token");
        assert_header(discovery, "x-oauth-header", "metadata-flow");
        assert_header(token, "x-oauth-header", "metadata-flow");
        assert!(
            !requests
                .iter()
                .any(|request| request.method == "POST" && request.path == "/oauth/register"),
            "URL-based client metadata flow should not hit dynamic registration",
        );
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn perform_oauth_login_rejects_invalid_client_metadata_url() {
        let server = spawn_test_oauth_server(true).await;

        let error = match perform_oauth_login_return_url(
            "server",
            &server.url,
            OAuthCredentialsStoreMode::File,
            None,
            None,
            &["profile".to_string()],
            None,
            None,
            Some("http://client.example.com"),
            Some(30),
            None,
            None,
        )
        .await
        {
            Ok(_) => panic!("invalid metadata URL should fail"),
            Err(error) => error,
        };

        assert!(
            error
                .to_string()
                .contains("must use https and include a non-root path"),
            "unexpected error: {error:#}",
        );
    }
}
