"""默认构建入口的运行时交付回归：不能再发布缺少 Code Mode 宿主的 CLI。"""

from contextlib import nullcontext
import importlib.util
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
spec = importlib.util.spec_from_file_location("aicodex_build", ROOT / "build.py")
build = importlib.util.module_from_spec(spec)
spec.loader.exec_module(build)


class RuntimeBuildTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        for name, value in (
            ("REPO_ROOT", self.root),
            ("RUST_ROOT", self.root / "rust"),
            ("TS_ROOT", self.root / "cli"),
        ):
            patcher = patch.object(build, name, value)
            patcher.start()
            self.addCleanup(patcher.stop)
        patcher = patch.dict(os.environ, {"CARGO_TARGET_DIR": str(self.root / "cargo")})
        patcher.start()
        self.addCleanup(patcher.stop)

    def make_artifacts(self, target, *, host=True):
        directory = self.root / "cargo" / target / "release"
        directory.mkdir(parents=True, exist_ok=True)
        names = [build.CLI_BIN_NAME]
        if host:
            names.append(build.CODE_MODE_HOST_NAME)
        for name in names:
            path = directory / build.executable_name(name, target)
            path.write_text(f"{name}:{target}")
            path.chmod(0o755)

    def test_cli_build_selects_and_stages_both_binaries_on_all_platforms(self):
        for target in build.CLI_TARGETS:
            with (
                self.subTest(target=target),
                patch.object(build, "build_rust") as cargo,
            ):
                self.make_artifacts(target)
                output = build.build_codex_cli(
                    target=target, output_dir=self.root / target
                )
                cargo.assert_called_once_with(
                    profile="release",
                    target=target,
                    package=["codex-cli", "codex-code-mode-host"],
                    bin=["aicodex", "codex-code-mode-host"],
                    verbose=False,
                )
                host = output.parent / build.executable_name(
                    build.CODE_MODE_HOST_NAME, target
                )
                self.assertEqual(output.read_text(), f"aicodex:{target}")
                self.assertEqual(host.read_text(), f"codex-code-mode-host:{target}")

    def test_missing_host_does_not_replace_existing_cli(self):
        target = "aarch64-apple-darwin"
        self.make_artifacts(target, host=False)
        installed = self.root / "aicodex"
        installed.write_text("existing-runtime")
        with (
            patch.object(build, "build_rust"),
            self.assertRaisesRegex(RuntimeError, "Required runtime binary missing"),
        ):
            build.build_codex_cli(target=target)
        self.assertEqual(installed.read_text(), "existing-runtime")

    def test_install_keeps_host_beside_renamed_cli_contract(self):
        target = "x86_64-pc-windows-msvc"
        self.make_artifacts(target)
        with patch.object(build, "build_rust"):
            output = build.build_codex_cli(target=target, install=True)
        self.assertEqual(
            output.parent, self.root / "cli" / "vendor" / target / "aicodex"
        )
        self.assertTrue((output.parent / "codex-code-mode-host.exe").is_file())

    def test_multi_target_outputs_cannot_overwrite_other_architecture_host(self):
        targets = ("aarch64-apple-darwin", "x86_64-apple-darwin")
        for target in targets:
            self.make_artifacts(target)
        with patch.object(build, "build_rust"):
            outputs = build.build_codex_cli_targets(targets=targets)
        for target, output in zip(targets, outputs, strict=True):
            self.assertEqual(output.parent, self.root / "dist" / target)
            self.assertEqual(
                (output.parent / "codex-code-mode-host").read_text(),
                f"codex-code-mode-host:{target}",
            )

    def test_host_filename_cannot_be_used_as_cli_rename(self):
        with patch.object(build, "build_rust") as cargo:
            with self.assertRaisesRegex(ValueError, "conflicts"):
                build.build_codex_cli(rename="codex-code-mode-host")
            cargo.assert_not_called()

    def test_rename_preserves_fixed_host_filename(self):
        target = "aarch64-apple-darwin"
        self.make_artifacts(target)
        with patch.object(build, "build_rust"):
            output = build.build_codex_cli(target=target, rename="custom-cli")
        self.assertEqual(output.name, "custom-cli")
        self.assertTrue((output.parent / "codex-code-mode-host").is_file())


class CargoInvocationTest(unittest.TestCase):
    def test_build_includes_host_and_verified_v8_environment(self):
        # 不运行 Cargo；观察默认入口真实组装的命令及传给子进程的依赖环境。
        with patch.dict(os.environ, {"CODEX_REPO_ROOT": str(ROOT)}):
            import scripts.codex_package.v8  # noqa: F401
        with (
            patch.object(build, "cargo_available", return_value=True),
            patch.object(
                build, "cargo_build_env", return_value={"RUSTC_WRAPPER": "sccache"}
            ),
            patch.object(
                build, "patched_rust_workspace_version", return_value=nullcontext()
            ),
            patch(
                "scripts.codex_package.v8.resolve_codex_v8_cargo_env",
                return_value={
                    "RUSTY_V8_ARCHIVE": "/verified/archive",
                    "RUSTY_V8_SRC_BINDING_PATH": "/verified/binding",
                },
            ),
            patch.object(build, "run") as run,
        ):
            build.build_rust(
                target="aarch64-apple-darwin",
                package=["codex-cli", "codex-code-mode-host"],
                bin=["aicodex", "codex-code-mode-host"],
            )
        run.assert_called_once_with(
            [
                "cargo",
                "build",
                "--release",
                "-p",
                "codex-cli",
                "-p",
                "codex-code-mode-host",
                "--bin",
                "aicodex",
                "--bin",
                "codex-code-mode-host",
                "--target",
                "aarch64-apple-darwin",
            ],
            cwd=build.RUST_ROOT,
            env={
                "RUSTC_WRAPPER": "sccache",
                "RUSTY_V8_ARCHIVE": "/verified/archive",
                "RUSTY_V8_SRC_BINDING_PATH": "/verified/binding",
            },
        )


if __name__ == "__main__":
    unittest.main()
