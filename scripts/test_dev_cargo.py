"""Validate the routine Cargo entrypoint without compiling or downloading."""

import contextlib
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(ROOT))
from scripts import dev_cargo  # noqa: E402


class DevCargoTest(unittest.TestCase):
    def command(self, *argv):
        args, forwarded = dev_cargo.parse_args(list(argv))
        return dev_cargo.cargo_command(args, forwarded)

    def test_runtime_group_is_identical_for_every_action(self):
        expected = [
            "codex-core",
            "codex-app-server",
            "codex-cli",
            "codex-code-mode-host",
            "codex-rmcp-client",
        ]
        for action in ("check", "build", "test"):
            with self.subTest(action=action):
                command = self.command(action)
                self.assertEqual(
                    [
                        command[i + 1]
                        for i, value in enumerate(command)
                        if value == "-p"
                    ],
                    expected,
                )
                self.assertIn("--locked", command)
                self.assertNotIn("--profile", command)
                self.assertNotIn("--cargo-profile", command)
                self.assertNotIn("--workspace", command)

    def test_workspace_group_is_explicit_for_every_action(self):
        for action in ("check", "build", "test"):
            command = self.command(action, "--group", "workspace")
            self.assertIn("--workspace", command)
            self.assertNotIn("-p", command)

    def test_default_and_filtered_check_targets(self):
        self.assertIn("--all-targets", self.command("check"))
        for selector in ("--lib", "--tests", "--bin=aicodex", "--test=smoke"):
            command = self.command("check", "--", selector)
            self.assertNotIn("--all-targets", command)
            self.assertEqual(command[-1], selector)
        self.assertNotIn("--all-targets", self.command("build"))

    def test_debug_profile_is_explicit_and_uses_nextest_cargo_option(self):
        for action in ("check", "build", "test"):
            command = self.command(action, "--profile", "dev-debug")
            option = "--cargo-profile" if action == "test" else "--profile"
            self.assertEqual(command[command.index(option) + 1], "dev-debug")

    def test_filters_are_forwarded_without_shell_interpretation(self):
        filters = ["--lib", "-E", "test(session) | test(message with spaces)"]
        self.assertEqual(self.command("test", "--", *filters)[-len(filters) :], filters)
        self.assertEqual(
            self.command("test", "--", "-Etest(profile)")[-1], "-Etest(profile)"
        )

    def test_configuration_overrides_are_rejected(self):
        options = [
            "-p",
            "-pcodex-core",
            "-vpcodex-core",
            "--package=codex-core",
            "--workspace",
            "--all",
            "--exclude=codex-cli",
            "--profile=release",
            "-Pci",
            "--cargo-profile=dev",
            "--release",
            "-r",
            "--target=custom",
            "--target-dir=/tmp/other",
            "--manifest-path=/tmp/Cargo.toml",
            "--config=build.target='custom'",
            "--features=custom",
            "-Fcustom",
            "--all-features",
            "--no-default-features",
            "-C/tmp",
            "-Zunstable-options",
        ]
        for option in options:
            with (
                self.subTest(option=option),
                contextlib.redirect_stderr(io.StringIO()) as stderr,
            ):
                with self.assertRaises(SystemExit) as failure:
                    self.command("test", "--", option)
                self.assertEqual(failure.exception.code, 2)
                self.assertIn("run Cargo directly", stderr.getvalue())

    def test_wrapper_options_are_not_abbreviated_or_accepted_after_separator(self):
        for argv in (("check", "--gro", "workspace"), ("test", "--lib")):
            with (
                contextlib.redirect_stderr(io.StringIO()),
                self.assertRaises(SystemExit),
            ):
                self.command(*argv)

    def test_dry_run_does_not_resolve_v8_or_start_any_process(self):
        for action in ("check", "build", "test"):
            with (
                patch.object(dev_cargo, "require_default_host_target") as host_check,
                patch.object(dev_cargo, "resolve_v8_env") as resolve,
                patch.object(dev_cargo.subprocess, "run") as run,
                contextlib.redirect_stdout(io.StringIO()) as stdout,
            ):
                self.assertEqual(dev_cargo.main([action, "--dry-run"]), 0)
            resolve.assert_not_called()
            host_check.assert_not_called()
            run.assert_not_called()
            plan = json.loads(stdout.getvalue())
            self.assertEqual(plan["cwd"], str(ROOT / "codex-rs"))
            self.assertEqual(plan["argv"], self.command(action))
            self.assertNotIn("SCCACHE_CACHE_SIZE", plan["env_overrides"])

    def test_actual_run_cwd_environment_and_error_code(self):
        with (
            patch.object(dev_cargo, "require_default_host_target"),
            patch.dict(
                os.environ,
                {
                    "RUSTC_WRAPPER": "custom-cache",
                    "RUST_MIN_STACK": "1",
                    "NEXTEST_PROFILE": "ci",
                },
            ),
            patch.object(
                dev_cargo,
                "resolve_v8_env",
                return_value={"RUSTY_V8_ARCHIVE": "/fixture/v8"},
            ) as resolve,
            patch.object(
                dev_cargo.subprocess,
                "run",
                return_value=subprocess.CompletedProcess([], 17),
            ) as run,
        ):
            self.assertEqual(dev_cargo.main(["test", "--", "--lib"]), 17)
            self.assertEqual(os.environ["RUST_MIN_STACK"], "1")
        resolve.assert_called_once()
        self.assertEqual(
            run.call_args.args[0][:5],
            ["cargo", "nextest", "run", "--no-fail-fast", "--locked"],
        )
        self.assertEqual(run.call_args.kwargs["cwd"], ROOT / "codex-rs")
        env = run.call_args.kwargs["env"]
        self.assertEqual(env["RUST_MIN_STACK"], "33554432")
        self.assertEqual(env["NEXTEST_PROFILE"], "local")
        self.assertEqual(env["RUSTC_WRAPPER"], "custom-cache")
        self.assertEqual(env["RUSTY_V8_ARCHIVE"], "/fixture/v8")
        self.assertFalse(run.call_args.kwargs["check"])

    def test_resolver_failure_never_starts_cargo(self):
        with (
            patch.object(dev_cargo, "require_default_host_target"),
            patch.object(
                dev_cargo, "resolve_v8_env", side_effect=RuntimeError("bad V8 checksum")
            ),
            patch.object(dev_cargo.subprocess, "run") as run,
            contextlib.redirect_stderr(io.StringIO()) as stderr,
        ):
            self.assertEqual(dev_cargo.main(["build"]), 1)
        run.assert_not_called()
        self.assertIn("bad V8 checksum", stderr.getvalue())

    def test_v8_uses_official_helper_and_restores_repository_environment(self):
        with patch.dict(os.environ, {"CODEX_REPO_ROOT": str(ROOT)}):
            import build
            from scripts.codex_package import targets, v8

        target = "aarch64-apple-darwin"
        environment = {"CUSTOM_ENV": "preserved"}
        artifacts = {
            "RUSTY_V8_ARCHIVE": "/archive",
            "RUSTY_V8_SRC_BINDING_PATH": "/binding",
        }
        with (
            patch.dict(os.environ, {"CODEX_REPO_ROOT": "/unrelated/caller"}),
            patch.object(build, "detect_cli_target", return_value=target),
            patch.object(
                v8, "resolve_codex_v8_cargo_env", return_value=artifacts
            ) as resolve,
        ):
            self.assertEqual(dev_cargo.resolve_v8_env(environment), artifacts)
            self.assertEqual(os.environ["CODEX_REPO_ROOT"], "/unrelated/caller")
        resolve.assert_called_once_with(
            targets.TARGET_SPECS[target], environ=environment
        )

    def test_missing_cargo_and_interrupt_return_errors(self):
        for error, code in (
            (FileNotFoundError("cargo missing"), 1),
            (KeyboardInterrupt(), 130),
        ):
            with (
                patch.object(dev_cargo, "require_default_host_target"),
                patch.object(dev_cargo, "resolve_v8_env", return_value={}),
                patch.object(dev_cargo.subprocess, "run", side_effect=error),
                contextlib.redirect_stderr(io.StringIO()),
            ):
                self.assertEqual(dev_cargo.main(["check"]), code)

    def test_explicit_target_environment_fails_before_v8_resolution(self):
        with (
            patch.dict(os.environ, {"CARGO_BUILD_TARGET": "aarch64-apple-darwin"}),
            patch.object(dev_cargo, "resolve_v8_env") as resolve,
            patch.object(dev_cargo.subprocess, "run") as run,
            contextlib.redirect_stderr(io.StringIO()) as stderr,
        ):
            self.assertEqual(dev_cargo.main(["check"]), 1)
        resolve.assert_not_called()
        run.assert_not_called()
        self.assertIn("CARGO_BUILD_TARGET", stderr.getvalue())

    def test_cargo_target_declarations_are_rejected_without_merging(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            rust_root = root / "repo" / "codex-rs"
            cargo_home = root / "cargo-home"
            for directory in (
                cargo_home,
                root / "repo" / ".cargo",
                rust_root / ".cargo",
            ):
                directory.mkdir(parents=True)
            with patch.object(dev_cargo, "RUST_ROOT", rust_root):
                for directory in (
                    cargo_home,
                    root / "repo" / ".cargo",
                    rust_root / ".cargo",
                ):
                    for name in ("config", "config.toml"):
                        with self.subTest(directory=directory, name=name):
                            path = directory / name
                            path.write_text(
                                '[build]\ntarget = "aarch64-apple-darwin"\n'
                            )
                            with self.assertRaisesRegex(
                                RuntimeError, "build.target is declared"
                            ):
                                dev_cargo.require_default_host_target(
                                    {"CARGO_HOME": str(cargo_home)}
                                )
                            path.unlink()
                (cargo_home / "config.toml").write_text(
                    '[build]\nrustc-wrapper = "sccache"\n'
                )
                dev_cargo.require_default_host_target({"CARGO_HOME": str(cargo_home)})


if __name__ == "__main__":
    unittest.main()
