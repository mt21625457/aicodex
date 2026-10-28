use std::ffi::OsString;
use std::path::Path;
use std::sync::Mutex;
use std::sync::MutexGuard;
use std::sync::OnceLock;
use std::sync::PoisonError;

use tempfile::tempdir;

/// Build the actual client's Stop-policy route pool before measuring protocol deadlines.
/// Client construction loads platform/custom CA state and can be slow on developer hosts.
/// Use a separate local endpoint so warmup cannot affect the test server's request assertions;
/// the same production HTTP capability still applies all proxy and certificate policy.
pub(crate) async fn warm_http_client(
    client: &dyn codex_exec_server::HttpClient,
) -> anyhow::Result<()> {
    let server = wiremock::MockServer::start().await;
    client
        .http_request(codex_exec_server::HttpRequestParams {
            method: "GET".to_string(),
            url: server.uri(),
            headers: Vec::new(),
            body: None,
            timeout_ms: None,
            redirect_policy: codex_exec_server::HttpRedirectPolicy::Stop,
            request_id: "test-client-warmup".to_string(),
            stream_response: false,
        })
        .await?;
    Ok(())
}

/// Serializes tests that mutate process-wide AICODEX_HOME and CODEX_HOME.
///
/// Keep OAuth tests on this one guard instead of defining per-module helpers; otherwise
/// concurrently running test modules can point File/Secrets storage at different homes.
pub(crate) struct TempCodexHome {
    _guard: MutexGuard<'static, ()>,
    _dir: tempfile::TempDir,
    previous_aicodex_home: Option<OsString>,
    previous_codex_home: Option<OsString>,
}

impl TempCodexHome {
    pub(crate) fn new() -> Self {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        let guard = LOCK
            .get_or_init(Mutex::default)
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let dir = tempdir().expect("create CODEX_HOME temp dir");
        let previous_aicodex_home = std::env::var_os("AICODEX_HOME");
        let previous_codex_home = std::env::var_os("CODEX_HOME");
        unsafe {
            std::env::set_var("AICODEX_HOME", dir.path());
            std::env::set_var("CODEX_HOME", dir.path());
        }
        Self {
            _guard: guard,
            _dir: dir,
            previous_aicodex_home,
            previous_codex_home,
        }
    }

    pub(super) fn path(&self) -> &Path {
        self._dir.path()
    }
}

impl Drop for TempCodexHome {
    fn drop(&mut self) {
        unsafe {
            for (name, previous) in [
                ("AICODEX_HOME", self.previous_aicodex_home.as_ref()),
                ("CODEX_HOME", self.previous_codex_home.as_ref()),
            ] {
                match previous {
                    Some(value) => std::env::set_var(name, value),
                    None => std::env::remove_var(name),
                }
            }
        }
    }
}

#[test]
fn temporary_home_isolates_both_home_environment_variables() -> anyhow::Result<()> {
    let home = TempCodexHome::new();
    assert_eq!(
        std::env::var_os("AICODEX_HOME"),
        Some(home.path().as_os_str().to_os_string())
    );
    assert_eq!(
        std::env::var_os("CODEX_HOME"),
        Some(home.path().as_os_str().to_os_string())
    );
    assert_eq!(
        codex_utils_home_dir::find_codex_home()?.as_path(),
        home.path().canonicalize()?.as_path()
    );
    Ok(())
}
