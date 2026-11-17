"""保护 SDK 测试的认证写入边界；所有宿主数据均由临时 canary 模拟。"""

import json
from pathlib import Path

import pytest
from app_server_harness import AppServerHarness

from openai_codex import Codex


def test_api_key_login_preserves_ambient_home(tmp_path, monkeypatch) -> None:
    """继承的高优先级 home 不得接收测试登录，模型请求必须到达模拟服务。"""
    ambient = tmp_path / "ambient-home"
    ambient.mkdir()
    auth = ambient / "auth.json"
    original = '{"auth_mode":"apikey","OPENAI_API_KEY":"fixture-original"}'
    auth.write_text(original)
    ambient_sqlite = tmp_path / "ambient-sqlite"
    ambient_sqlite.mkdir()
    monkeypatch.setenv("AICODEX_HOME", str(ambient))
    monkeypatch.setenv("CODEX_SQLITE_HOME", str(ambient_sqlite))
    monkeypatch.setenv("OPENAI_API_KEY", "fixture-ambient")
    with AppServerHarness(tmp_path, requires_openai_auth=True) as harness:
        # 即使旧实现逃逸，仍只会读写临时目录并访问本地模拟服务。
        (ambient / "config.toml").write_bytes((harness.codex_home / "config.toml").read_bytes())
        harness.responses.enqueue_assistant_message("isolated login", response_id="isolation")
        with Codex(config=harness.app_server_config()) as codex:
            codex.login_api_key("fixture-login")
            assert auth.read_text() == original
            saved = json.loads((harness.codex_home / "auth.json").read_text())
            assert saved["OPENAI_API_KEY"] == "fixture-login"
            assert codex.thread_start().run("check isolation").final_response == "isolated login"
            assert (
                harness.responses.single_request().header("authorization") == "Bearer fixture-login"
            )
        assert auth.read_text() == original
        assert list(ambient_sqlite.iterdir()) == []


def test_harness_requires_checkout_binary_without_installed_fallback(tmp_path, monkeypatch) -> None:
    """没有明确的被测二进制时应在启动前失败，不能猜测已安装运行时的隔离能力。"""
    monkeypatch.delenv("CODEX_EXEC_PATH", raising=False)
    monkeypatch.setattr(Path, "is_file", lambda self: False)
    with AppServerHarness(tmp_path) as harness:
        with pytest.raises(FileNotFoundError, match="CODEX_EXEC_PATH"):
            harness.app_server_config()


def test_harness_rejects_empty_binary_override(tmp_path, monkeypatch) -> None:
    """空的显式覆盖属于错误配置，不能转换成默认运行时。"""
    monkeypatch.setenv("CODEX_EXEC_PATH", "")
    with AppServerHarness(tmp_path) as harness:
        with pytest.raises(ValueError, match="CODEX_EXEC_PATH"):
            harness.app_server_config()


def test_harness_isolates_both_runtime_home_names(tmp_path, monkeypatch) -> None:
    """Mock app-server tests must not inherit either runtime home from the host."""
    monkeypatch.setenv("AICODEX_HOME", str(tmp_path / "unrelated-home"))
    monkeypatch.setenv("CODEX_HOME", str(tmp_path / "another-unrelated-home"))
    with AppServerHarness(tmp_path) as harness:
        config = harness.app_server_config()
        assert config.env["AICODEX_HOME"] == str(harness.codex_home)
        assert config.env["CODEX_HOME"] == str(harness.codex_home)
        assert config.env["CODEX_SQLITE_HOME"] == str(harness.codex_home)
        assert config.env["OPENAI_API_KEY"] == ""
        assert (
            'model_provider = "mock_provider"' in (harness.codex_home / "config.toml").read_text()
        )
