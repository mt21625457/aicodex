#!/usr/bin/env python3
"""Run routine Cargo validation with a stable package and feature selection.

Examples:
    python3 scripts/dev_cargo.py check --dry-run
    python3 scripts/dev_cargo.py test -- --lib -E 'test(session)'
    python3 scripts/dev_cargo.py build --group workspace --profile dev-debug

This entrypoint uses Cargo's default host mode without a target-triple directory.
Use Cargo directly for custom packages, features, targets, or release builds.
"""

from __future__ import annotations

import argparse
import json
import os
from pathlib import Path
import subprocess
import sys


REPO_ROOT = Path(__file__).resolve().parents[1]
RUST_ROOT = REPO_ROOT / "codex-rs"
RUNTIME_PACKAGES = (
    "codex-core",
    "codex-app-server",
    "codex-cli",
    "codex-code-mode-host",
    "codex-rmcp-client",
)
BLOCKED_OPTIONS = {
    "--package",
    "--workspace",
    "--all",
    "--exclude",
    "--profile",
    "--cargo-profile",
    "--release",
    "--target",
    "--target-dir",
    "--manifest-path",
    "--config",
    "--features",
    "--all-features",
    "--no-default-features",
}
TARGET_OPTIONS = {
    "--lib",
    "--bin",
    "--bins",
    "--test",
    "--tests",
    "--example",
    "--examples",
    "--bench",
    "--benches",
    "--all-targets",
}


def parse_args(argv: list[str]) -> tuple[argparse.Namespace, list[str]]:
    parser = argparse.ArgumentParser(description=__doc__, allow_abbrev=False)
    parser.add_argument("action", choices=("check", "build", "test"))
    parser.add_argument("--group", choices=("runtime", "workspace"), default="runtime")
    parser.add_argument("--profile", choices=("dev-debug",))
    parser.add_argument(
        "--dry-run",
        action="store_true",
        help="print JSON without running tools or resolving V8",
    )
    parser.epilog = "Pass Cargo/nextest filters after --. Custom packages/features/targets require direct Cargo."
    split = argv.index("--") if "--" in argv else len(argv)
    args = parser.parse_args(argv[:split])
    forwarded = argv[split + 1 :]
    for value in forwarded:
        # Reject compact short options as well (e.g. -pcodex-core, -vpfoo).
        # Do not interpret attached -E filter expressions as option clusters.
        option = value.split("=", 1)[0]
        short_override = (
            value.startswith("-")
            and not value.startswith("--")
            and value[1:].lstrip("vq")[:1] in {"p", "P", "r", "F", "C", "Z"}
        )
        if option in BLOCKED_OPTIONS or short_override:
            parser.error(
                f"forwarded argument {value!r} can change the fixed package, feature, "
                "profile or target configuration; use --group/--profile before --, "
                "or run Cargo directly for custom builds"
            )
    return args, forwarded


def cargo_command(args: argparse.Namespace, forwarded: list[str]) -> list[str]:
    if args.action == "test":
        command = ["cargo", "nextest", "run", "--no-fail-fast"]
    else:
        command = ["cargo", args.action]
    command.append("--locked")
    if args.group == "workspace":
        command.append("--workspace")
    else:
        for package in RUNTIME_PACKAGES:
            command.extend(["-p", package])
    if args.profile:
        command.extend(
            [
                "--cargo-profile" if args.action == "test" else "--profile",
                args.profile,
            ]
        )
    if args.action == "check" and not any(
        value.split("=", 1)[0] in TARGET_OPTIONS for value in forwarded
    ):
        command.append("--all-targets")
    return [*command, *forwarded]


def resolve_v8_env(environ: dict[str, str]) -> dict[str, str]:
    # Imports are intentionally delayed until after dry-run. The package helper
    # reads CODEX_REPO_ROOT at import time; preserve the caller's environment.
    sys.path.insert(0, str(REPO_ROOT))
    previous_root = os.environ.get("CODEX_REPO_ROOT")
    os.environ["CODEX_REPO_ROOT"] = str(REPO_ROOT)
    try:
        from build import detect_cli_target
        from scripts.codex_package.targets import TARGET_SPECS
        from scripts.codex_package.v8 import resolve_codex_v8_cargo_env
    finally:
        sys.path.pop(0)
        if previous_root is None:
            os.environ.pop("CODEX_REPO_ROOT", None)
        else:
            os.environ["CODEX_REPO_ROOT"] = previous_root
    target = detect_cli_target()
    if target not in TARGET_SPECS:
        raise RuntimeError(f"Unsupported V8 Cargo target: {target}")
    return resolve_codex_v8_cargo_env(TARGET_SPECS[target], environ=environ)


def require_default_host_target(environ: dict[str, str]) -> None:
    """Reject target declarations conservatively, without merging Cargo config."""
    if "CARGO_BUILD_TARGET" in environ:
        raise RuntimeError(
            "dev-cargo is host-only: CARGO_BUILD_TARGET is set. Unset it or run "
            "Cargo directly for an explicit target (including the host triple)."
        )

    import tomllib

    cargo_home = Path(environ.get("CARGO_HOME") or Path.home() / ".cargo")
    if not cargo_home.is_absolute():
        cargo_home = RUST_ROOT / cargo_home
    directories = [
        cargo_home,
        *(path / ".cargo" for path in (RUST_ROOT, *RUST_ROOT.parents)),
    ]
    for directory in dict.fromkeys(directories):
        # Inspect both names deliberately: even a shadowed target declaration
        # requires an explicit direct-Cargo workflow, instead of guessing policy.
        for name in ("config", "config.toml"):
            path = directory / name
            if not path.is_file():
                continue
            with path.open("rb") as source:
                config = tomllib.load(source)
            if "target" in config.get("build", {}):
                raise RuntimeError(
                    f"dev-cargo is host-only: build.target is declared in {path}. "
                    "Remove this declaration or run Cargo directly for an explicit "
                    "target (including the host triple)."
                )


def main(argv: list[str] | None = None) -> int:
    args, forwarded = parse_args(sys.argv[1:] if argv is None else argv)
    command = cargo_command(args, forwarded)
    overrides = {}
    if args.action == "test":
        overrides.update(RUST_MIN_STACK="33554432", NEXTEST_PROFILE="local")
    if args.dry_run:
        print(
            json.dumps(
                {
                    "cwd": str(RUST_ROOT),
                    "argv": command,
                    "env_overrides": overrides,
                    "target_policy": "host-only; execution rejects CARGO_BUILD_TARGET and Cargo build.target declarations",
                    "v8": "resolved only during execution using the official artifact helper",
                },
                indent=2,
            )
        )
        return 0

    environ = os.environ.copy()
    environ.update(overrides)
    try:
        require_default_host_target(environ)
        environ.update(resolve_v8_env(environ))
        result = subprocess.run(command, cwd=RUST_ROOT, env=environ, check=False)
        return result.returncode if result.returncode >= 0 else 128 - result.returncode
    except KeyboardInterrupt:
        return 130
    except (OSError, RuntimeError, ValueError) as error:
        print(f"dev-cargo: {error}", file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
