#![cfg(debug_assertions)]

use anyhow::Result;
use predicates::str::contains;
use std::path::Path;
use tempfile::TempDir;

fn codex_command(codex_home: &Path) -> Result<assert_cmd::Command> {
    let mut cmd = assert_cmd::Command::new(codex_utils_cargo_bin::cargo_bin("aicodex")?);
    cmd.env("AICODEX_HOME", codex_home);
    cmd.env("AICODEX_HOME", codex_home)
        .env("CODEX_HOME", codex_home);
    cmd.env_remove("CODEX_SQLITE_HOME");
    Ok(cmd)
}

#[tokio::test]
async fn update_does_not_start_interactive_prompt() -> Result<()> {
    let codex_home = TempDir::new()?;

    codex_command(codex_home.path())?
        .arg("update")
        .assert()
        .failure()
        .stderr(contains("`codex update` is not available in debug builds"));

    Ok(())
}
