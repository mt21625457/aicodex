#!/usr/bin/env python3
"""
Build script for the Codex project.

Supports building:
- Rust components (via Cargo)
- TypeScript/Node.js components (via pnpm)
- Native binaries for CLI distribution

Usage:
    ./build.py                     # Default: build all in release mode, output binary named "aicodex"
    ./build.py --help
    ./build.py rust                # Build all Rust crates (release mode)
    ./build.py rust --debug        # Build Rust in debug mode
    ./build.py ts                  # Build TypeScript packages
    ./build.py all                 # Build everything (release mode)
    ./build.py all --debug         # Build everything in debug mode
    ./build.py codex-cli           # Build the aicodex CLI binary (release mode)
    ./build.py codex-cli --all-targets
"""

from __future__ import annotations

import argparse
import os
import platform
import re
import shutil
import subprocess
import sys
from contextlib import contextmanager
from pathlib import Path
from typing import Iterator

# Repository root (where this script lives)
REPO_ROOT = Path(__file__).resolve().parent
RUST_ROOT = REPO_ROOT / "codex-rs"
TS_ROOT = REPO_ROOT / "codex-cli"
AICODEX_VERSION_FILE = REPO_ROOT / "AICODEX_VERSION"

# Output binary name
OUTPUT_BINARY_NAME = "aicodex"
CLI_PACKAGE_NAME = "codex-cli"
CLI_BIN_NAME = "aicodex"
CLI_VENDOR_DIR_NAME = "aicodex"

CLI_TARGETS = (
    "x86_64-unknown-linux-musl",
    "aarch64-unknown-linux-musl",
    "x86_64-apple-darwin",
    "aarch64-apple-darwin",
    "x86_64-pc-windows-msvc",
    "aarch64-pc-windows-msvc",
)


def read_aicodex_version() -> str:
    """Read the product version embedded into Rust binaries."""
    try:
        version = AICODEX_VERSION_FILE.read_text(encoding="utf-8").strip()
    except FileNotFoundError:
        print(f"ERROR: version file not found: {AICODEX_VERSION_FILE}", file=sys.stderr)
        sys.exit(1)

    if not version:
        print(f"ERROR: version file is empty: {AICODEX_VERSION_FILE}", file=sys.stderr)
        sys.exit(1)

    if any(ch.isspace() for ch in version):
        print(
            f"ERROR: version file must contain a single version token: {AICODEX_VERSION_FILE}",
            file=sys.stderr,
        )
        sys.exit(1)

    if any(ch in version for ch in ('"', "\\")):
        print(
            f"ERROR: version contains characters that cannot be written to Cargo.toml: {version!r}",
            file=sys.stderr,
        )
        sys.exit(1)

    if not re.fullmatch(
        r"\d+\.\d+\.\d+(?:-[0-9A-Za-z.-]+)?(?:\+[0-9A-Za-z.-]+)?",
        version,
    ):
        print(
            f"ERROR: version must be a Cargo-compatible semver value: {version!r}",
            file=sys.stderr,
        )
        sys.exit(1)

    return version


def _cargo_toml_with_workspace_version(text: str, version: str) -> str:
    lines = text.splitlines(keepends=True)
    in_workspace_package = False

    for index, line in enumerate(lines):
        stripped = line.strip()
        if stripped == "[workspace.package]":
            in_workspace_package = True
            continue

        if in_workspace_package and stripped.startswith("["):
            break

        if in_workspace_package and stripped.startswith("version"):
            prefix = line[: len(line) - len(line.lstrip())]
            newline = "\n" if line.endswith("\n") else ""
            lines[index] = f'{prefix}version = "{version}"{newline}'
            return "".join(lines)

    raise RuntimeError("Could not find [workspace.package] version in codex-rs/Cargo.toml")


@contextmanager
def patched_rust_workspace_version(version: str) -> Iterator[None]:
    """Temporarily patch Cargo metadata so env!(\"CARGO_PKG_VERSION\") is correct."""
    cargo_toml = RUST_ROOT / "Cargo.toml"
    cargo_lock = RUST_ROOT / "Cargo.lock"
    original_toml = cargo_toml.read_text(encoding="utf-8")
    patched_toml = _cargo_toml_with_workspace_version(original_toml, version)
    original_lock = cargo_lock.read_bytes() if cargo_lock.exists() else None

    if patched_toml != original_toml:
        print(f"  → Embedding AICODEX_VERSION={version}", file=sys.stderr)
        cargo_toml.write_text(patched_toml, encoding="utf-8")

    try:
        yield
    finally:
        cargo_toml.write_text(original_toml, encoding="utf-8")
        if original_lock is None:
            if cargo_lock.exists():
                cargo_lock.unlink()
        else:
            cargo_lock.write_bytes(original_lock)


def run(
    cmd: list[str],
    *,
    cwd: Path | None = None,
    env: dict[str, str] | None = None,
    check: bool = True,
    capture_output: bool = False,
) -> subprocess.CompletedProcess[str]:
    """Run a shell command and stream output."""
    merged_env = {**os.environ, **(env or {})}
    print(f"  → {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(
        cmd,
        cwd=cwd,
        env=merged_env,
        check=check,
        capture_output=capture_output,
        text=True,
    )


def cargo_available() -> bool:
    return shutil.which("cargo") is not None


def pnpm_available() -> bool:
    return shutil.which("pnpm") is not None


def node_available() -> bool:
    return shutil.which("node") is not None


def detect_cli_target() -> str:
    """Detect the target triple used by the npm aicodex launcher."""
    system = platform.system().lower()
    machine = platform.machine().lower()

    mapping = {
        ("linux", "x86_64"): "x86_64-unknown-linux-musl",
        ("linux", "aarch64"): "aarch64-unknown-linux-musl",
        ("darwin", "x86_64"): "x86_64-apple-darwin",
        ("darwin", "arm64"): "aarch64-apple-darwin",
        ("windows", "amd64"): "x86_64-pc-windows-msvc",
        ("windows", "x86_64"): "x86_64-pc-windows-msvc",
        ("windows", "arm64"): "aarch64-pc-windows-msvc",
    }

    key = (system, machine)
    if key in mapping:
        return mapping[key]

    raise RuntimeError(f"Unsupported platform for aicodex npm package: {system} ({machine})")


def executable_name(base_name: str, target: str) -> str:
    """Return the executable filename for a Rust target triple."""
    return f"{base_name}.exe" if "windows" in target else base_name


# ---------------------------------------------------------------------------
# Rust builds
# ---------------------------------------------------------------------------

def build_rust(
    *,
    release: bool = True,
    target: str | None = None,
    package: str | None = None,
    bin: str | None = None,
    features: list[str] | None = None,
    jobs: int | None = None,
    verbose: bool = False,
) -> None:
    """Build Rust workspace using Cargo."""
    if not cargo_available():
        print("ERROR: cargo not found. Install Rust: https://rustup.rs/", file=sys.stderr)
        sys.exit(1)

    cmd: list[str] = ["cargo", "build"]
    if release:
        cmd.append("--release")
    if package:
        cmd += ["-p", package]
    if bin:
        cmd += ["--bin", bin]
    if target:
        cmd += ["--target", target]
    if features:
        cmd += ["--features", ",".join(features)]
    if jobs:
        cmd += ["-j", str(jobs)]
    if verbose:
        cmd.append("--verbose")

    version = read_aicodex_version()
    with patched_rust_workspace_version(version):
        run(cmd, cwd=RUST_ROOT)


def build_codex_cli(
    *,
    release: bool = True,
    target: str | None = None,
    install: bool = False,
    rename: str | None = None,
    verbose: bool = False,
) -> Path:
    """Build the aicodex CLI binary and optionally stage / rename it."""
    resolved_target = target or detect_cli_target()
    build_rust(
        release=release,
        target=resolved_target,
        package=CLI_PACKAGE_NAME,
        bin=CLI_BIN_NAME,
        verbose=verbose,
    )

    profile = "release" if release else "debug"
    src = (
        RUST_ROOT
        / "target"
        / resolved_target
        / profile
        / executable_name(CLI_BIN_NAME, resolved_target)
    )

    if not src.exists():
        print(f"ERROR: expected binary not found at {src}", file=sys.stderr)
        sys.exit(1)

    if install and rename and rename != CLI_BIN_NAME:
        raise RuntimeError(f"--install requires the binary to be named {CLI_BIN_NAME!r}")

    if install:
        dest_name = executable_name(CLI_BIN_NAME, resolved_target)
    elif rename:
        dest_name = executable_name(rename, resolved_target)
    else:
        dest_name = src.name

    if install:
        # Stage the binary into codex-cli/vendor/<target>/aicodex/ so the JS
        # wrapper can find it when running from the local repo.
        legacy_dest_dir = TS_ROOT / "vendor" / resolved_target / "codex"
        if legacy_dest_dir.exists():
            print(f"  → Removing legacy vendor directory {legacy_dest_dir}", file=sys.stderr)
            shutil.rmtree(legacy_dest_dir)

        dest_dir = TS_ROOT / "vendor" / resolved_target / CLI_VENDOR_DIR_NAME
        dest_dir.mkdir(parents=True, exist_ok=True)
        dest = dest_dir / dest_name

        print(f"  → Staging {src} → {dest}", file=sys.stderr)
        shutil.copy2(src, dest)
        return dest

    # Also copy to repo root with the desired name for easy access
    dest = REPO_ROOT / dest_name
    print(f"  → Copying {src} → {dest}", file=sys.stderr)
    shutil.copy2(src, dest)
    return dest


def build_codex_cli_targets(
    *,
    targets: tuple[str, ...],
    release: bool = True,
    install: bool = False,
    rename: str | None = None,
    verbose: bool = False,
) -> list[Path]:
    """Build the aicodex CLI binary for multiple target triples."""
    outputs = []
    for target in targets:
        target_rename = None
        if not install:
            base_name = rename or CLI_BIN_NAME
            target_rename = f"{base_name}-{target}"

        outputs.append(
            build_codex_cli(
                release=release,
                target=target,
                install=install,
                rename=target_rename,
                verbose=verbose,
            )
        )
    return outputs


# ---------------------------------------------------------------------------
# TypeScript builds
# ---------------------------------------------------------------------------

def build_ts(*, install_deps: bool = True, verbose: bool = False) -> None:
    """Build TypeScript packages using pnpm."""
    if not node_available():
        print("ERROR: node not found. Install Node.js >= 22.", file=sys.stderr)
        sys.exit(1)

    if not pnpm_available():
        print("ERROR: pnpm not found. Install pnpm >= 10.33.0.", file=sys.stderr)
        sys.exit(1)

    if install_deps:
        run(["pnpm", "install"], cwd=REPO_ROOT)

    print("TypeScript dependencies installed.", file=sys.stderr)


# ---------------------------------------------------------------------------
# Meta builds
# ---------------------------------------------------------------------------

def build_all(
    *,
    release: bool = True,
    target: str | None = None,
    all_targets: bool = False,
    install_cli: bool = False,
    rename: str | None = None,
    verbose: bool = False,
) -> None:
    """Build Rust + TypeScript components."""
    build_ts(install_deps=True, verbose=verbose)
    if all_targets:
        build_codex_cli_targets(
            targets=CLI_TARGETS,
            release=release,
            install=install_cli,
            rename=rename,
            verbose=verbose,
        )
    else:
        build_codex_cli(
            release=release,
            target=target,
            install=install_cli,
            rename=rename,
            verbose=verbose,
        )


# ---------------------------------------------------------------------------
# CLI
# ---------------------------------------------------------------------------

def main(argv: list[str] | None = None) -> None:
    parser = argparse.ArgumentParser(
        prog="build.py",
        description="Build the Codex monorepo.",
    )
    sub = parser.add_subparsers(dest="command", required=False)

    # rust
    rust_parser = sub.add_parser("rust", help="Build Rust workspace (release mode by default)")
    rust_parser.add_argument("--debug", action="store_true", help="Build in debug mode instead of release")
    rust_parser.add_argument("--target", default=None, help="Rust target triple")
    rust_parser.add_argument("-p", "--package", default=None, help="Build specific package")
    rust_parser.add_argument("--bin", default=None, help="Build specific binary")
    rust_parser.add_argument("--features", nargs="+", default=None, help="Enable features")
    rust_parser.add_argument("-j", "--jobs", type=int, default=None, help="Build jobs")

    # codex-cli
    cli_parser = sub.add_parser("codex-cli", help="Build the aicodex CLI binary (release mode by default)")
    cli_parser.add_argument("--debug", action="store_true", help="Build in debug mode instead of release")
    cli_target_group = cli_parser.add_mutually_exclusive_group()
    cli_target_group.add_argument("--target", default=None, help="Rust target triple")
    cli_target_group.add_argument(
        "--all-targets",
        action="store_true",
        help="Build all supported npm targets",
    )
    cli_parser.add_argument(
        "--install",
        action="store_true",
        help="Stage binary into codex-cli/vendor for local use",
    )
    cli_parser.add_argument(
        "--rename",
        default=None,
        help="Rename the output binary",
    )

    # ts
    ts_parser = sub.add_parser("ts", help="Build TypeScript packages")
    ts_parser.add_argument(
        "--no-install",
        action="store_true",
        help="Skip pnpm install",
    )

    # all
    all_parser = sub.add_parser("all", help="Build everything (release mode by default)")
    all_parser.add_argument("--debug", action="store_true", help="Build in debug mode instead of release")
    all_target_group = all_parser.add_mutually_exclusive_group()
    all_target_group.add_argument("--target", default=None, help="Rust target triple")
    all_target_group.add_argument(
        "--all-targets",
        action="store_true",
        help="Build all supported npm targets",
    )
    all_parser.add_argument(
        "--install-cli",
        action="store_true",
        help="Stage CLI binary into codex-cli/vendor",
    )
    all_parser.add_argument(
        "--rename",
        default=None,
        help="Rename the output binary",
    )

    # Common flags
    for p in (rust_parser, cli_parser, all_parser, ts_parser):
        p.add_argument("-v", "--verbose", action="store_true", help="Verbose output")

    args = parser.parse_args(argv)

    # Default behavior: build all in release mode, rename to "aicodex"
    if args.command is None:
        build_all(release=True, rename=OUTPUT_BINARY_NAME)
        print("Build completed successfully.", file=sys.stderr)
        return

    if args.command == "rust":
        build_rust(
            release=not args.debug,
            target=args.target,
            package=args.package,
            bin=args.bin,
            features=args.features,
            jobs=args.jobs,
            verbose=args.verbose,
        )
    elif args.command == "codex-cli":
        if args.all_targets:
            build_codex_cli_targets(
                targets=CLI_TARGETS,
                release=not args.debug,
                install=args.install,
                rename=args.rename or OUTPUT_BINARY_NAME,
                verbose=args.verbose,
            )
        else:
            build_codex_cli(
                release=not args.debug,
                target=args.target,
                install=args.install,
                rename=args.rename or OUTPUT_BINARY_NAME,
                verbose=args.verbose,
            )
    elif args.command == "ts":
        build_ts(install_deps=not args.no_install, verbose=args.verbose)
    elif args.command == "all":
        build_all(
            release=not args.debug,
            target=args.target,
            all_targets=args.all_targets,
            install_cli=args.install_cli,
            rename=args.rename or OUTPUT_BINARY_NAME,
            verbose=args.verbose,
        )
    else:
        parser.print_help()
        sys.exit(1)

    print("Build completed successfully.", file=sys.stderr)


if __name__ == "__main__":
    main()
