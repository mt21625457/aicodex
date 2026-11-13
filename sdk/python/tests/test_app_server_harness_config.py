"""Keep mock app-server tests isolated from the developer's runtime home."""

from app_server_harness import AppServerHarness


def test_harness_isolates_both_runtime_home_names(tmp_path, monkeypatch) -> None:
    monkeypatch.setenv("AICODEX_HOME", str(tmp_path / "unrelated-home"))
    with AppServerHarness(tmp_path) as harness:
        config = harness.app_server_config()
        assert config.env["AICODEX_HOME"] == str(harness.codex_home)
        assert config.env["CODEX_HOME"] == str(harness.codex_home)
        assert (
            'model_provider = "mock_provider"' in (harness.codex_home / "config.toml").read_text()
        )
