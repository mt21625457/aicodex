#!/usr/bin/env python3
"""
Build script for the Codex project.

Supports building:
- Rust components (via Cargo)
- TypeScript/Node.js components (via pnpm)
- Native binaries for CLI distribution

Usage:
    ./build.py                     # Default: build all in minimized release mode, output binary named "aicodex"
    ./build.py --help
    ./build.py rust                # Build all Rust crates in minimized release mode
    ./build.py rust --fast         # Build Rust in fast local mode
    ./build.py ts                  # Build TypeScript packages
    ./build.py all                 # Build everything in minimized release mode
    ./build.py all --fast          # Build everything in fast local mode
    ./build.py codex-cli           # Build the aicodex CLI binary in minimized release mode
    ./build.py codex-cli --fast    # Build the aicodex CLI binary in fast local mode
    ./build.py codex-cli --all-targets
    ./build.py clean --dry-run     # Show build artifacts that can be removed
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

FAST_BUILD_PROFILE = "dev-small"
RELEASE_BUILD_PROFILE = "release"
DEFAULT_BUILD_PROFILE = RELEASE_BUILD_PROFILE
DEBUG_BUILD_PROFILE = "dev"

MINIMIZED_RELEASE_PROFILE_ENV = {
    "CARGO_PROFILE_RELEASE_OPT_LEVEL": "z",
    "CARGO_PROFILE_RELEASE_LTO": "fat",
    "CARGO_PROFILE_RELEASE_CODEGEN_UNITS": "1",
    "CARGO_PROFILE_RELEASE_DEBUG": "none",
    "CARGO_PROFILE_RELEASE_SPLIT_DEBUGINFO": "off",
    "CARGO_PROFILE_RELEASE_STRIP": "symbols",
    "CARGO_PROFILE_RELEASE_PANIC": "abort",
}

CLEAN_BUILD_DIRS = (
    REPO_ROOT / "dist",
    REPO_ROOT / "build",
    REPO_ROOT / "out",
    REPO_ROOT / "storybook-static",
    REPO_ROOT / ".cache",
    REPO_ROOT / ".turbo",
    REPO_ROOT / ".parcel-cache",
    REPO_ROOT / ".jest",
    REPO_ROOT / ".nyc_output",
    REPO_ROOT / "coverage",
    REPO_ROOT / "bazel-bin",
    REPO_ROOT / "bazel-out",
    REPO_ROOT / "bazel-testlogs",
    REPO_ROOT / "bazel-aicodex",
)

CLEAN_TS_DEP_DIRS = (
    REPO_ROOT / "node_modules",
    REPO_ROOT / ".pnpm-store",
)

CLEAN_VENDOR_DIRS = (
    TS_ROOT / "vendor",
)

CLEAN_TARGETS = ("build", "rust", "ts", "deps", "vendor", "all")


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
    resolved_cmd = _resolve_windows_command(cmd, merged_env)
    print(f"  → {' '.join(cmd)}", file=sys.stderr)
    return subprocess.run(
        resolved_cmd,
        cwd=cwd,
        env=merged_env,
        check=check,
        capture_output=capture_output,
        text=True,
    )


def sccache_wrapper_command() -> str | None:
    """Return a portable sccache wrapper command when available."""
    for name in ("sccache", "sccache.exe"):
        if shutil.which(name):
            return name

    return None


def is_sccache_wrapper(wrapper: str | None) -> bool:
    """Return true when a Rust compiler wrapper points to sccache."""
    return bool(wrapper) and Path(wrapper).stem.lower() == "sccache"


def require_sccache_wrapper_command() -> str:
    """Return the sccache wrapper command or exit with a helpful error."""
    if os.environ.get("SCCACHE_DISABLE"):
        print(
            "ERROR: build.py requires sccache caching, but SCCACHE_DISABLE is set. "
            "Unset SCCACHE_DISABLE and rerun the build.",
            file=sys.stderr,
        )
        sys.exit(1)

    wrapper = os.environ.get("RUSTC_WRAPPER")
    if wrapper and is_sccache_wrapper(wrapper):
        command = sccache_wrapper_command()
        if command:
            return command

        expanded = Path(wrapper).expanduser()
        if expanded.is_file() and os.access(expanded, os.X_OK):
            return wrapper

        print(f"ERROR: RUSTC_WRAPPER points to sccache but is not executable: {wrapper}", file=sys.stderr)
        sys.exit(1)

    if wrapper:
        print(
            f"  → Overriding RUSTC_WRAPPER={wrapper!r}; build.py requires sccache",
            file=sys.stderr,
        )

    command = sccache_wrapper_command()
    if command:
        return command

    print(
        "ERROR: build.py requires sccache, but it was not found. "
        "Install it first, for example: cargo install --locked sccache",
        file=sys.stderr,
    )
    sys.exit(1)


def cargo_build_env(profile: str) -> dict[str, str]:
    """Return Cargo environment overrides for minimized, cached builds."""
    wrapper = require_sccache_wrapper_command()
    env: dict[str, str] = {
        "RUSTC_WRAPPER": wrapper,
        # Incremental artifacts are not cacheable by sccache and get very large
        # in this workspace, so build.py always prefers the shared compiler cache.
        "CARGO_INCREMENTAL": "0",
    }

    if profile == RELEASE_BUILD_PROFILE:
        env.update(MINIMIZED_RELEASE_PROFILE_ENV)

    return env


def cargo_profile_args(profile: str) -> list[str]:
    """Return Cargo CLI flags for a profile name."""
    if profile == DEBUG_BUILD_PROFILE:
        return []
    if profile == RELEASE_BUILD_PROFILE:
        return ["--release"]
    return ["--profile", profile]


def cargo_profile_output_dir(profile: str) -> str:
    """Return Cargo's output directory name for a profile."""
    if profile == DEBUG_BUILD_PROFILE:
        return "debug"
    return profile


def selected_profile(args: argparse.Namespace) -> str:
    return args.profile or DEFAULT_BUILD_PROFILE


def add_cargo_profile_args(parser: argparse.ArgumentParser) -> None:
    group = parser.add_mutually_exclusive_group()
    group.add_argument(
        "--fast",
        dest="profile",
        action="store_const",
        const=FAST_BUILD_PROFILE,
        help=f"Build with the fast local Cargo profile ({FAST_BUILD_PROFILE}); development only",
    )
    group.add_argument(
        "--debug",
        dest="profile",
        action="store_const",
        const=DEBUG_BUILD_PROFILE,
        help="Build with Cargo's dev/debug profile",
    )
    group.add_argument(
        "--release",
        dest="profile",
        action="store_const",
        const=RELEASE_BUILD_PROFILE,
        help="Build with the minimized Cargo release profile; default",
    )
    group.add_argument(
        "--profile",
        dest="profile",
        default=None,
        help="Build with a specific Cargo profile",
    )


def cargo_available() -> bool:
    return shutil.which("cargo") is not None


def pnpm_available() -> bool:
    return shutil.which("pnpm") is not None


def node_available() -> bool:
    return shutil.which("node") is not None


def _resolve_windows_command(cmd: list[str], env: dict[str, str]) -> list[str]:
    """Resolve command shims that CreateProcess cannot find by bare name."""
    if os.name != "nt" or not cmd:
        return cmd

    path = None
    for key in reversed(list(env)):
        if key.upper() == "PATH":
            path = env[key]
            break

    resolved = shutil.which(cmd[0], path=path)
    if resolved is None:
        return cmd

    return [resolved, *cmd[1:]]


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
# Cleaning
# ---------------------------------------------------------------------------

def repo_relative(path: Path) -> str:
    """Return a path display string relative to the repository when possible."""
    try:
        return str(path.resolve().relative_to(REPO_ROOT))
    except ValueError:
        return str(path)


def remove_path(path: Path, *, dry_run: bool) -> bool:
    """Remove a file, symlink, or directory if it exists."""
    if not path.exists() and not path.is_symlink():
        return False

    action = "Would remove" if dry_run else "Removing"
    print(f"  → {action} {repo_relative(path)}", file=sys.stderr)
    if dry_run:
        return True

    if path.is_dir() and not path.is_symlink():
        shutil.rmtree(path)
    else:
        path.unlink()
    return True


def root_binary_paths() -> list[Path]:
    """Return known top-level binaries produced by this build script."""
    paths = [REPO_ROOT / OUTPUT_BINARY_NAME, REPO_ROOT / f"{OUTPUT_BINARY_NAME}.exe"]
    for target in CLI_TARGETS:
        paths.append(REPO_ROOT / executable_name(f"{OUTPUT_BINARY_NAME}-{target}", target))

    seen = set()
    unique_paths = []
    for path in paths:
        if path not in seen:
            unique_paths.append(path)
            seen.add(path)
    return unique_paths


def rust_target_dirs() -> list[Path]:
    """Return Cargo target directories known to this workspace."""
    target_dirs = [RUST_ROOT / "target"]
    target_dirs.extend(sorted(RUST_ROOT.glob("target-*")))
    return target_dirs


def clean_rust_outputs(*, dry_run: bool, target: str | None = None) -> None:
    """Clean Rust build outputs while respecting Cargo target selection."""
    if target:
        cmd = ["cargo", "clean", "--target", target]
        if dry_run:
            print(f"  → Would run {' '.join(cmd)} in {repo_relative(RUST_ROOT)}", file=sys.stderr)
            return
        if cargo_available():
            result = run(cmd, cwd=RUST_ROOT, check=False)
            if result.returncode == 0:
                return
            print(
                "  → cargo clean failed; falling back to removing the target directory",
                file=sys.stderr,
            )
        remove_path(RUST_ROOT / "target" / target, dry_run=dry_run)
        return

    if cargo_available():
        cmd = ["cargo", "clean"]
        if dry_run:
            print(f"  → Would run {' '.join(cmd)} in {repo_relative(RUST_ROOT)}", file=sys.stderr)
        else:
            result = run(cmd, cwd=RUST_ROOT, check=False)
            if result.returncode != 0:
                print(
                    "  → cargo clean failed; falling back to removing Cargo target directories",
                    file=sys.stderr,
                )

    for path in rust_target_dirs():
        remove_path(path, dry_run=dry_run)


def clean_build_outputs(
    *,
    dry_run: bool,
    target: str | None = None,
    include_root_binaries: bool = True,
) -> None:
    """Clean generated build artifacts, keeping dependency directories intact."""
    clean_rust_outputs(dry_run=dry_run, target=target)
    for path in CLEAN_BUILD_DIRS:
        remove_path(path, dry_run=dry_run)
    if include_root_binaries:
        for path in root_binary_paths():
            remove_path(path, dry_run=dry_run)


def clean_ts_outputs(*, dry_run: bool) -> None:
    """Clean TypeScript build/cache outputs without deleting installed deps."""
    for path in CLEAN_BUILD_DIRS:
        remove_path(path, dry_run=dry_run)


def clean_dependency_dirs(*, dry_run: bool) -> None:
    """Clean repository-local dependency directories."""
    for path in CLEAN_TS_DEP_DIRS:
        remove_path(path, dry_run=dry_run)


def prune_pnpm_store(*, dry_run: bool) -> None:
    """Prune pnpm's store when pnpm is available."""
    cmd = ["pnpm", "store", "prune"]
    if dry_run:
        print(f"  → Would run {' '.join(cmd)}", file=sys.stderr)
        return
    if not pnpm_available():
        print("  → Skipping pnpm store prune: pnpm not found", file=sys.stderr)
        return
    run(cmd, cwd=REPO_ROOT)


def clean_vendor_outputs(*, dry_run: bool) -> None:
    """Clean native binaries staged for npm packages."""
    for path in CLEAN_VENDOR_DIRS:
        remove_path(path, dry_run=dry_run)


def clean_requested_targets(
    targets: list[str],
    *,
    dry_run: bool = False,
    target: str | None = None,
    prune_store: bool = False,
    include_root_binaries: bool = True,
) -> None:
    """Clean the requested artifact groups."""
    selected = list(targets or ["build"])
    unknown = sorted(set(selected) - set(CLEAN_TARGETS))
    if unknown:
        raise ValueError(f"Unknown clean target(s): {', '.join(unknown)}")

    if "all" in selected:
        selected = ["build", "deps", "vendor"]

    print(f"Cleaning targets: {', '.join(selected)}", file=sys.stderr)
    if dry_run:
        print("Dry run only; no files will be removed.", file=sys.stderr)

    if "build" in selected:
        clean_build_outputs(
            dry_run=dry_run,
            target=target,
            include_root_binaries=include_root_binaries,
        )
    else:
        if "rust" in selected:
            clean_rust_outputs(dry_run=dry_run, target=target)
        if "ts" in selected:
            clean_ts_outputs(dry_run=dry_run)

    if "deps" in selected:
        clean_dependency_dirs(dry_run=dry_run)
        if prune_store:
            prune_pnpm_store(dry_run=dry_run)

    if "vendor" in selected:
        clean_vendor_outputs(dry_run=dry_run)


def clean_targets_for_build_command(command: str | None) -> list[str]:
    """Return the artifact group to clean around a build command."""
    if command == "rust":
        return ["rust"]
    if command == "ts":
        return ["ts"]
    return ["build"]


# ---------------------------------------------------------------------------
# Rust builds
# ---------------------------------------------------------------------------

def build_rust(
    *,
    profile: str = DEFAULT_BUILD_PROFILE,
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
    cmd += cargo_profile_args(profile)
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
    cargo_env = cargo_build_env(profile)
    print(f"  → Using sccache: {cargo_env['RUSTC_WRAPPER']}", file=sys.stderr)
    if profile == RELEASE_BUILD_PROFILE:
        print("  → Enforcing minimized release profile", file=sys.stderr)
    with patched_rust_workspace_version(version):
        run(cmd, cwd=RUST_ROOT, env=cargo_env)


def build_codex_cli(
    *,
    profile: str = DEFAULT_BUILD_PROFILE,
    target: str | None = None,
    install: bool = False,
    rename: str | None = None,
    verbose: bool = False,
) -> Path:
    """Build the aicodex CLI binary and optionally stage / rename it."""
    resolved_target = target or detect_cli_target()
    build_rust(
        profile=profile,
        target=resolved_target,
        package=CLI_PACKAGE_NAME,
        bin=CLI_BIN_NAME,
        verbose=verbose,
    )

    src = (
        RUST_ROOT
        / "target"
        / resolved_target
        / cargo_profile_output_dir(profile)
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
    profile: str = DEFAULT_BUILD_PROFILE,
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
                profile=profile,
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
    profile: str = DEFAULT_BUILD_PROFILE,
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
            profile=profile,
            install=install_cli,
            rename=rename,
            verbose=verbose,
        )
    else:
        build_codex_cli(
            profile=profile,
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
    rust_parser = sub.add_parser("rust", help=f"Build Rust workspace ({DEFAULT_BUILD_PROFILE} by default)")
    add_cargo_profile_args(rust_parser)
    rust_parser.add_argument("--target", default=None, help="Rust target triple")
    rust_parser.add_argument("-p", "--package", default=None, help="Build specific package")
    rust_parser.add_argument("--bin", default=None, help="Build specific binary")
    rust_parser.add_argument("--features", nargs="+", default=None, help="Enable features")
    rust_parser.add_argument("-j", "--jobs", type=int, default=None, help="Build jobs")

    # codex-cli
    cli_parser = sub.add_parser(
        "codex-cli",
        help=f"Build the aicodex CLI binary ({DEFAULT_BUILD_PROFILE} by default)",
    )
    add_cargo_profile_args(cli_parser)
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
    all_parser = sub.add_parser("all", help=f"Build everything ({DEFAULT_BUILD_PROFILE} by default)")
    add_cargo_profile_args(all_parser)
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

    # clean
    clean_parser = sub.add_parser("clean", help="Clean build artifacts and dependency directories")
    clean_parser.add_argument(
        "targets",
        nargs="*",
        choices=CLEAN_TARGETS,
        default=["build"],
        help="Artifact groups to clean: build, rust, ts, deps, vendor, all",
    )
    clean_parser.add_argument(
        "--dry-run",
        action="store_true",
        help="Print what would be removed without deleting anything",
    )
    clean_parser.add_argument(
        "--target",
        default=None,
        help="Rust target triple to pass to cargo clean",
    )
    clean_parser.add_argument(
        "--prune-pnpm-store",
        action="store_true",
        help="Run pnpm store prune when cleaning deps",
    )

    # Common flags
    for p in (rust_parser, cli_parser, all_parser, ts_parser):
        p.add_argument("-v", "--verbose", action="store_true", help="Verbose output")
        p.add_argument(
            "--clean-before",
            action="store_true",
            help="Clean build artifacts before running this build",
        )
        p.add_argument(
            "--clean-after",
            action="store_true",
            help="Clean intermediate build artifacts after a successful build",
        )

    args = parser.parse_args(argv)

    # Default behavior: build all in minimized release mode, rename to "aicodex"
    if args.command is None:
        build_all(profile=DEFAULT_BUILD_PROFILE, rename=OUTPUT_BINARY_NAME)
        print("Build completed successfully.", file=sys.stderr)
        return

    build_clean_target = getattr(args, "target", None)
    if getattr(args, "all_targets", False):
        build_clean_target = None

    command_clean_targets = clean_targets_for_build_command(args.command)
    if getattr(args, "clean_before", False):
        clean_requested_targets(command_clean_targets, target=build_clean_target)

    if args.command == "rust":
        build_rust(
            profile=selected_profile(args),
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
                profile=selected_profile(args),
                install=args.install,
                rename=args.rename or OUTPUT_BINARY_NAME,
                verbose=args.verbose,
            )
        else:
            build_codex_cli(
                profile=selected_profile(args),
                target=args.target,
                install=args.install,
                rename=args.rename or OUTPUT_BINARY_NAME,
                verbose=args.verbose,
            )
    elif args.command == "ts":
        build_ts(install_deps=not args.no_install, verbose=args.verbose)
    elif args.command == "all":
        build_all(
            profile=selected_profile(args),
            target=args.target,
            all_targets=args.all_targets,
            install_cli=args.install_cli,
            rename=args.rename or OUTPUT_BINARY_NAME,
            verbose=args.verbose,
        )
    elif args.command == "clean":
        clean_requested_targets(
            args.targets,
            dry_run=args.dry_run,
            target=args.target,
            prune_store=args.prune_pnpm_store,
        )
        print("Clean completed successfully.", file=sys.stderr)
        return
    else:
        parser.print_help()
        sys.exit(1)

    if getattr(args, "clean_after", False):
        clean_requested_targets(
            command_clean_targets,
            target=build_clean_target,
            include_root_binaries=False,
        )

    print("Build completed successfully.", file=sys.stderr)


if __name__ == "__main__":
    main()
