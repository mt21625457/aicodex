#!/usr/bin/env python3
"""Inspect build storage; explicitly prune old profile contents under codex-rs/target.

No deps-level garbage collection. Cargo's profile directory and .cargo-lock inode
survive pruning so processes already waiting on the lock cannot bypass it.
Only macOS/Linux support apply. sccache inspection never contacts its daemon.
"""

import argparse
from contextlib import ExitStack, contextmanager
import json
import math
import os
from pathlib import Path
import re
import stat
import sys
import time

PROFILES = ("debug", "dev-small", "dev-debug", "release", "profiling", "ci-test")
TRIPLE = re.compile(r"[A-Za-z0-9_]+(?:-[A-Za-z0-9_]+){2,}")
REPO = Path(__file__).resolve().parent.parent


class CacheError(Exception):
    pass


def directory(path):
    """Reject symlinks, including ancestors; do not resolve into shared targets."""
    if path.absolute() != path.resolve():
        raise CacheError(f"symlink path is unsupported: {path}")
    if not stat.S_ISDIR(path.lstat().st_mode):
        raise CacheError(f"not a directory: {path}")


def open_directory(path):
    """Anchor every absolute path component, not just the final directory."""
    path = path.absolute()
    fd = os.open(path.anchor, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW)
    try:
        for component in path.parts[1:]:
            next_fd = os.open(
                component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd
            )
            os.close(fd)
            fd = next_fd
        return fd
    except BaseException:
        os.close(fd)
        raise


def discover(target):
    """Only standard profiles, target triples, and an explicit analyzer namespace."""
    found = []
    if not target.exists() and not target.is_symlink():
        return found
    directory(target)
    bases = [(target, False)]
    analyzer = target / "rust-analyzer"
    if analyzer.exists() or analyzer.is_symlink():
        directory(analyzer)
        bases.append((analyzer, True))
    for base, is_analyzer in bases:
        parents = [base]
        for child in sorted(base.iterdir()):
            if TRIPLE.fullmatch(child.name) and child.name not in PROFILES:
                if child.is_symlink():
                    raise CacheError(f"symlink target triple is unsupported: {child}")
                if child.is_dir():
                    parents.append(child)
        for parent in parents:
            for profile in PROFILES:
                path = parent / profile
                if path.exists() or path.is_symlink():
                    directory(path)
                    found.append(
                        {
                            "path": str(path.relative_to(target)),
                            "profile": profile,
                            "rust_analyzer": is_analyzer,
                        }
                    )
    return sorted(found, key=lambda item: item["path"])


def measure(path, seen=None):
    """lstat allocation accounting; hard links count once, symlinks never followed."""
    seen = set() if seen is None else seen
    allocated = 0
    latest = 0.0

    def account(info):
        nonlocal allocated, latest
        latest = max(latest, info.st_mtime)
        key = (info.st_dev, info.st_ino)
        if key not in seen:
            if not hasattr(info, "st_blocks"):
                raise CacheError(
                    "allocated-byte accounting is unavailable on this platform"
                )
            seen.add(key)
            allocated += info.st_blocks * 512

    def walk(fd):
        for name in os.listdir(fd):
            info = os.stat(name, dir_fd=fd, follow_symlinks=False)
            account(info)
            if stat.S_ISDIR(info.st_mode):
                child = os.open(
                    name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd
                )
                try:
                    walk(child)
                finally:
                    os.close(child)

    info = path.lstat()
    account(info)
    if stat.S_ISDIR(info.st_mode):
        fd = open_directory(path)
        try:
            walk(fd)
        finally:
            os.close(fd)
    return {"allocated_bytes": allocated, "newest_mtime": latest}


def lock_status(path):
    if sys.platform not in ("darwin", "linux"):
        return "unsupported"
    import fcntl

    try:
        fd = os.open(path, os.O_RDONLY | os.O_NOFOLLOW | os.O_NONBLOCK)
    except FileNotFoundError:
        return "absent"
    try:
        if not stat.S_ISREG(os.fstat(fd).st_mode):
            raise CacheError(f"invalid Cargo lock: {path}")
        try:
            fcntl.flock(fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            return "busy"
        fcntl.flock(fd, fcntl.LOCK_UN)
        return "available (snapshot only)"
    finally:
        os.close(fd)


def sccache_report():
    """Read local config/disk only: --show-stats may start a missing server."""
    home = Path.home()
    if sys.platform == "darwin":
        config = home / "Library/Application Support/Mozilla.sccache/config"
        cache = home / "Library/Caches/Mozilla.sccache"
    else:
        config = (
            Path(os.environ.get("XDG_CONFIG_HOME", home / ".config")) / "sccache/config"
        )
        cache = Path(os.environ.get("XDG_CACHE_HOME", home / ".cache")) / "sccache"
    config = Path(os.environ.get("SCCACHE_CONF", config))
    result = {"scope": "shared; read-only; daemon not queried", "config": str(config)}
    disk = {}
    if config.exists():
        try:
            import tomllib

            with config.open("rb") as stream:
                disk = tomllib.load(stream).get("cache", {}).get("disk", {})
        except (ImportError, OSError, ValueError, AttributeError) as error:
            result["config_error"] = str(error)
    cache = Path(os.environ.get("SCCACHE_DIR", disk.get("dir", cache))).expanduser()
    result["directory"] = str(cache)
    result["configured_limit"] = os.environ.get(
        "SCCACHE_CACHE_SIZE", disk.get("size", "unknown (sccache default not queried)")
    )
    if cache.exists():
        try:
            directory(cache)
            result["allocated_bytes"] = measure(cache)["allocated_bytes"]
        except (OSError, CacheError) as error:
            result["error"] = str(error)
    else:
        result["status"] = "directory absent"
    return result


def report(target, *, now=None):
    now = time.time() if now is None else now
    candidates = discover(target)
    for item in candidates:
        path = target / item["path"]
        size = measure(path)
        item.update(
            allocated_bytes=size["allocated_bytes"],
            age_days=max(0, (now - size["newest_mtime"]) / 86400),
            cargo_lock=lock_status(path / ".cargo-lock"),
        )
    total = measure(target)["allocated_bytes"] if target.exists() else 0
    return {
        "target": str(target),
        "total_allocated_bytes": total,
        "profiles": candidates,
        "rust_analyzer_allocated_bytes": measure(target / "rust-analyzer")[
            "allocated_bytes"
        ]
        if (target / "rust-analyzer").exists()
        else 0,
        "cargo_target_dir_env": os.environ.get("CARGO_TARGET_DIR"),
        "accounting": "st_blocks * 512; hardlinks deduplicated within each row and across total; rows can overlap; not a guaranteed reclaim estimate",
        "sccache": sccache_report(),
    }


@contextmanager
def locked_profile(target_fd, relative):
    """Open every component without following links; retain the original lock inode."""
    import fcntl

    with ExitStack() as stack:
        fd = target_fd
        target_device = os.fstat(target_fd).st_dev
        for component in Path(relative).parts:
            fd = os.open(
                component, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd
            )
            stack.callback(os.close, fd)
            if os.fstat(fd).st_dev != target_device:
                raise CacheError(f"mount point in profile path is unsupported: {relative}")
        root_before = os.fstat(fd)
        lock_flags = os.O_RDWR | os.O_NOFOLLOW | os.O_NONBLOCK
        created = False
        try:
            lock_fd = os.open(".cargo-lock", lock_flags, dir_fd=fd)
        except FileNotFoundError:
            lock_fd = os.open(
                ".cargo-lock", lock_flags | os.O_CREAT | os.O_EXCL, 0o600, dir_fd=fd
            )
            created = True
        stack.callback(os.close, lock_fd)
        info = os.fstat(lock_fd)
        if not stat.S_ISREG(info.st_mode) or info.st_nlink != 1:
            raise CacheError(f"unsafe Cargo lock: {relative}")
        try:
            fcntl.flock(lock_fd, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError as error:
            raise CacheError(f"active Cargo lock: {relative}") from error
        age_state = {
            "baseline": max(root_before.st_mtime, 0 if created else info.st_mtime),
            "created": created,
            "root_after_creation": os.fstat(fd).st_mtime,
        }
        yield fd, info, age_state
        # Closing the descriptor also releases the lock on exceptions/interrupts.


def inspect_fd(fd, *, root_device, root=True):
    """Recheck age and safety under the Cargo lock, without path-based traversal."""
    latest = 0.0
    for name in os.listdir(fd):
        if root and name == ".cargo-lock":
            continue
        info = os.stat(name, dir_fd=fd, follow_symlinks=False)
        latest = max(latest, info.st_mtime)
        if info.st_dev != root_device:
            raise CacheError("mount point inside profile is unsupported")
        if name == ".cargo-lock":
            raise CacheError("nested Cargo lock inside profile is unsupported")
        if stat.S_ISDIR(info.st_mode):
            child = os.open(
                name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd
            )
            try:
                latest = max(
                    latest, inspect_fd(child, root_device=root_device, root=False)
                )
            finally:
                os.close(child)
    return latest


def clear_fd(fd, *, root=True):
    for name in os.listdir(fd):
        if root and name == ".cargo-lock":
            continue
        info = os.stat(name, dir_fd=fd, follow_symlinks=False)
        if stat.S_ISDIR(info.st_mode):
            child = os.open(
                name, os.O_RDONLY | os.O_DIRECTORY | os.O_NOFOLLOW, dir_fd=fd
            )
            try:
                clear_fd(child, root=False)
            finally:
                os.close(child)
            os.rmdir(name, dir_fd=fd)
        else:
            os.unlink(name, dir_fd=fd)


def reject_linux_mounts(target):
    """Device IDs alone cannot detect same-filesystem Linux bind mounts."""
    if sys.platform != "linux":
        return
    for line in Path("/proc/self/mountinfo").read_text().splitlines():
        fields = line.split()
        if len(fields) < 6:
            raise CacheError("cannot parse Linux mount table safely")
        mount = Path(re.sub(r"\\([0-7]{3})", lambda m: chr(int(m[1], 8)), fields[4]))
        if mount == target or target in mount.parents:
            raise CacheError(f"mount point in target is unsupported: {mount}")


def prune(target, profiles, rust_analyzer, min_age_days, apply=False, *, now=None):
    if not profiles and not rust_analyzer:
        raise CacheError("prune requires --profile NAME and/or --rust-analyzer")
    if not math.isfinite(min_age_days) or min_age_days < 0:
        raise CacheError("--min-age-days must be finite and nonnegative")
    if apply and sys.platform not in ("darwin", "linux"):
        raise CacheError("apply supports only macOS/Linux Cargo flock locks")
    configured = os.environ.get("CARGO_TARGET_DIR")
    if apply and configured and Path(configured).resolve() != target.resolve():
        raise CacheError(
            "external CARGO_TARGET_DIR: apply unsupported; unset it to manage the standard repository target"
        )
    now = time.time() if now is None else now
    cutoff = now - min_age_days * 86400
    candidates = discover(target)
    result = {
        "mode": "apply" if apply else "dry-run",
        "target": str(target),
        "profiles": [],
    }
    eligible = []
    for item in candidates:
        selected = item["rust_analyzer"] if rust_analyzer else False
        if not item["rust_analyzer"]:
            selected = item["profile"] in profiles
        if not selected:
            continue
        path = target / item["path"]
        size = measure(path)
        item.update(
            allocated_bytes=size["allocated_bytes"],
            cargo_lock=lock_status(path / ".cargo-lock"),
            status="candidate"
            if size["newest_mtime"] <= cutoff
            else "retained (recent)",
        )
        result["profiles"].append(item)
        if item["status"] == "candidate":
            eligible.append(item)
    if not apply or not eligible:
        return result
    directory(target)
    reject_linux_mounts(target)
    with ExitStack() as stack:
        target_fd = open_directory(target)
        stack.callback(os.close, target_fd)
        parent_fd = open_directory(target.parent)
        stack.callback(os.close, parent_fd)
        target_device = os.fstat(target_fd).st_dev
        if target_device != os.fstat(parent_fd).st_dev:
            raise CacheError("mounted target directory is unsupported")
        ready = []
        for item in eligible:
            fd, lock_info, age_state = stack.enter_context(
                locked_profile(target_fd, item["path"])
            )
            path = target / item["path"]
            directory(path)
            opened = os.fstat(fd)
            current = path.lstat()
            if (opened.st_dev, opened.st_ino) != (current.st_dev, current.st_ino):
                raise CacheError(f"profile changed during locking: {path}")
            current_lock = os.stat(".cargo-lock", dir_fd=fd, follow_symlinks=False)
            if (current_lock.st_dev, current_lock.st_ino) != (
                lock_info.st_dev,
                lock_info.st_ino,
            ):
                raise CacheError(f"Cargo lock changed during locking: {path}")
            latest = max(age_state["baseline"], inspect_fd(fd, root_device=target_device))
            # Creating a missing lock changes the directory's mtime ourselves.
            # Exclude only those exact creation timestamps, not later activity.
            if not age_state["created"] or opened.st_mtime != age_state["root_after_creation"]:
                latest = max(latest, opened.st_mtime)
            if not age_state["created"] or current_lock.st_mtime != lock_info.st_mtime:
                latest = max(latest, current_lock.st_mtime)
            if latest > cutoff:
                item["status"] = "retained (changed during locking)"
            else:
                ready.append((item, fd))
        for item, fd in ready:
            try:
                clear_fd(fd)
            except OSError as error:
                raise CacheError(
                    f"partial cleanup of {item['path']}: {error}; earlier profiles may have been cleared; rerun report"
                ) from error
            item["status"] = "cleared (directory and Cargo lock retained)"
    return result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "command", nargs="?", choices=("report", "prune"), default="report"
    )
    parser.add_argument(
        "--profile",
        action="append",
        choices=PROFILES,
        default=[],
        help="select this profile under the standard target and any target triples",
    )
    parser.add_argument(
        "--rust-analyzer",
        action="store_true",
        help="select known profiles under target/rust-analyzer separately",
    )
    parser.add_argument(
        "--min-age-days",
        type=float,
        default=14,
        help="retain profiles with any entry newer than this many days (default: 14)",
    )
    parser.add_argument(
        "--apply",
        action="store_true",
        help="clear selected old profile contents, preserving Cargo lock inodes",
    )
    parser.add_argument("--json", action="store_true")
    args = parser.parse_args(argv)
    if args.command == "report" and (args.apply or args.profile or args.rust_analyzer):
        parser.error("--apply, --profile and --rust-analyzer require prune")
    target = REPO / "codex-rs/target"
    try:
        result = (
            report(target)
            if args.command == "report"
            else prune(
                target, args.profile, args.rust_analyzer, args.min_age_days, args.apply
            )
        )
        if args.json:
            print(json.dumps(result, indent=2))
        else:
            print(f"{result.get('mode', 'report')}: {target}")
            if "total_allocated_bytes" in result:
                print(f"Total allocated: {result['total_allocated_bytes']:,} bytes")
                print(
                    f"rust-analyzer total: {result['rust_analyzer_allocated_bytes']:,} bytes"
                )
                if result["cargo_target_dir_env"]:
                    print(
                        f"CARGO_TARGET_DIR={result['cargo_target_dir_env']} (report covers standard repository target only)"
                    )
                print(result["accounting"])
            for row in result["profiles"]:
                state = (
                    row["status"]
                    if "status" in row
                    else f"{row['age_days']:.1f} days old"
                )
                print(
                    f"  {row['path']}: {row['allocated_bytes']:,} bytes; {state}; lock={row['cargo_lock']}"
                )
            if "sccache" in result:
                print("sccache: " + json.dumps(result["sccache"], ensure_ascii=False))
            if args.command == "prune" and not args.apply:
                print(
                    "Dry-run only. No files changed; use --apply to clear eligible contents."
                )
        return 0
    except (CacheError, OSError) as error:
        if args.json:
            print(json.dumps({"error": str(error)}), file=sys.stderr)
        else:
            print(f"build-cache: {error}", file=sys.stderr)
        return 1
    except KeyboardInterrupt:
        print(
            "build-cache: interrupted; cleanup may be partial; Cargo locks released",
            file=sys.stderr,
        )
        return 130


if __name__ == "__main__":
    sys.exit(main())
