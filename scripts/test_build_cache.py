"""Storage tests use temporary directories only; never the repository target."""

import contextlib
import io
import json
import os
from pathlib import Path
import select
import subprocess
import sys
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest import mock

try:
    from scripts import build_cache as cache
except ModuleNotFoundError:
    import build_cache as cache


@unittest.skipUnless(sys.platform in ("darwin", "linux"), "POSIX allocation/locking")
class BuildCacheTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        # macOS /var is a symlink; production paths are canonical too.
        self.repo = Path(self.temp.name).resolve()
        self.target = self.repo / "codex-rs/target"
        self.target.mkdir(parents=True)
        self.now = time.time()
        self.env = mock.patch.dict(os.environ, {}, clear=False)
        self.env.start()
        os.environ.pop("CARGO_TARGET_DIR", None)
        self.addCleanup(self.env.stop)

    def profile(self, relative="debug", age=30, lock=True):
        path = self.target / relative
        (path / "deps").mkdir(parents=True)
        (path / "deps/artifact").write_bytes(b"data" * 2048)
        if lock:
            (path / ".cargo-lock").touch()
        self.age(path, age)
        return path

    def age(self, path, days):
        stamp = self.now - days * 86400
        for child in path.rglob("*"):
            os.utime(child, (stamp, stamp), follow_symlinks=False)
        os.utime(path, (stamp, stamp))

    def prune(self, **kwargs):
        options = dict(
            profiles=["debug"], rust_analyzer=False, min_age_days=14, now=self.now
        )
        options.update(kwargs)
        return cache.prune(self.target, **options)

    def test_default_cli_is_report_and_read_only(self):
        profile = self.profile(lock=False)
        output = io.StringIO()
        with (
            mock.patch.object(cache, "REPO", self.repo),
            mock.patch.object(cache, "sccache_report", return_value={}),
            contextlib.redirect_stdout(output),
        ):
            self.assertEqual(cache.main(["--json"]), 0)
        self.assertEqual(
            json.loads(output.getvalue())["profiles"][0]["cargo_lock"], "absent"
        )
        self.assertTrue((profile / "deps/artifact").exists())
        self.assertFalse((profile / ".cargo-lock").exists())

    def test_dry_run_never_creates_lock_or_deletes(self):
        profile = self.profile(lock=False)
        result = self.prune()
        self.assertEqual(result["mode"], "dry-run")
        self.assertEqual(result["profiles"][0]["status"], "candidate")
        self.assertTrue((profile / "deps/artifact").exists())
        self.assertFalse((profile / ".cargo-lock").exists())

    def test_apply_preserves_profile_and_lock_inode_and_other_profiles(self):
        profile = self.profile()
        release = self.profile("release")
        before = (profile.stat().st_ino, (profile / ".cargo-lock").stat().st_ino)
        self.prune(apply=True)
        self.assertEqual(list(profile.iterdir()), [profile / ".cargo-lock"])
        self.assertEqual(
            before, (profile.stat().st_ino, (profile / ".cargo-lock").stat().st_ino)
        )
        self.assertTrue((release / "deps/artifact").exists())

    def test_apply_can_create_missing_lock(self):
        profile = self.profile(lock=False)
        self.prune(apply=True)
        self.assertEqual(list(profile.iterdir()), [profile / ".cargo-lock"])

    def test_profile_selection_required(self):
        self.profile()
        with self.assertRaisesRegex(cache.CacheError, "requires"):
            self.prune(profiles=[], apply=True)

    def test_recent_descendant_retains_entire_profile(self):
        profile = self.profile()
        os.utime(profile / "deps/artifact", (self.now, self.now))
        result = self.prune(apply=True)
        self.assertEqual(result["profiles"][0]["status"], "retained (recent)")
        self.assertTrue((profile / "deps/artifact").exists())

    def test_invalid_retention_rejected(self):
        for days in (-1, float("nan"), float("inf")):
            with self.assertRaises(cache.CacheError):
                self.prune(min_age_days=days, apply=True)

    def test_activity_after_initial_scan_rechecked_under_lock(self):
        profile = self.profile()
        original = cache.locked_profile

        @contextlib.contextmanager
        def changed(*args):
            with original(*args) as locked:
                os.utime(profile / "deps/artifact", (self.now, self.now))
                yield locked

        with mock.patch.object(cache, "locked_profile", changed):
            result = self.prune(apply=True)
        self.assertEqual(
            result["profiles"][0]["status"], "retained (changed during locking)"
        )
        self.assertTrue((profile / "deps/artifact").exists())

    def test_root_and_existing_lock_activity_rechecked_under_lock(self):
        profile = self.profile()
        original = cache.locked_profile
        for changed_path in (profile, profile / ".cargo-lock"):
            self.age(profile, 30)

            @contextlib.contextmanager
            def changed(*args):
                with original(*args) as locked:
                    os.utime(changed_path, (self.now, self.now))
                    yield locked

            with mock.patch.object(cache, "locked_profile", changed):
                result = self.prune(apply=True)
            self.assertEqual(
                result["profiles"][0]["status"], "retained (changed during locking)"
            )
            self.assertTrue((profile / "deps/artifact").exists())

    def test_mounted_target_profile_and_triple_are_rejected(self):
        profile = self.profile("aarch64-unknown-linux-gnu/debug")
        original = os.fstat
        for mounted in (self.target, profile.parent, profile):
            inode = mounted.stat().st_ino

            def changed_device(fd):
                info = original(fd)
                if info.st_ino != inode:
                    return info
                values = {key: getattr(info, key) for key in dir(info) if key.startswith("st_")}
                values["st_dev"] += 1
                return SimpleNamespace(**values)

            with mock.patch.object(cache.os, "fstat", side_effect=changed_device):
                with self.assertRaisesRegex(cache.CacheError, "mount"):
                    self.prune(apply=True)
            self.assertTrue((profile / "deps/artifact").exists())

    def test_linux_bind_mount_table_rejects_same_device_mounts(self):
        mounted = self.target / "debug/bind mount"
        escaped = str(mounted).replace(" ", r"\040")
        table = f"1 2 8:1 /bind {escaped} rw - ext4 /dev/root rw\n"
        with (
            mock.patch.object(cache.sys, "platform", "linux"),
            mock.patch.object(Path, "read_text", return_value=table),
        ):
            with self.assertRaisesRegex(cache.CacheError, "mount point"):
                cache.reject_linux_mounts(self.target)

    def test_active_lock_aborts_before_any_profile_deletion(self):
        import fcntl

        debug = self.profile()
        release = self.profile("release")
        with (release / ".cargo-lock").open("rb") as handle:
            fcntl.flock(handle, fcntl.LOCK_EX | fcntl.LOCK_NB)
            self.assertEqual(cache.lock_status(release / ".cargo-lock"), "busy")
            with self.assertRaisesRegex(cache.CacheError, "active Cargo lock"):
                self.prune(profiles=["debug", "release"], apply=True)
        self.assertTrue((debug / "deps/artifact").exists())
        self.assertTrue((release / "deps/artifact").exists())
        self.assertEqual(
            cache.lock_status(debug / ".cargo-lock"), "available (snapshot only)"
        )

    def test_waiter_remains_blocked_through_cleanup_on_same_inode(self):
        profile = self.profile()
        inode = (profile / ".cargo-lock").stat().st_ino
        original = cache.clear_fd
        processes = []

        def clear(fd, **kwargs):
            if not kwargs.get("root", True):
                return original(fd, **kwargs)
            process = subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    "import fcntl,sys; f=open(sys.argv[1], 'rb'); print('opened',flush=True); fcntl.flock(f,fcntl.LOCK_EX); print('acquired',flush=True)",
                    str(profile / ".cargo-lock"),
                ],
                stdout=subprocess.PIPE,
                text=True,
            )
            processes.append(process)
            self.assertTrue(select.select([process.stdout], [], [], 5)[0])
            self.assertEqual(process.stdout.readline().strip(), "opened")
            original(fd, **kwargs)
            self.assertEqual((profile / ".cargo-lock").stat().st_ino, inode)
            self.assertFalse(select.select([process.stdout], [], [], 0.15)[0])

        try:
            with mock.patch.object(cache, "clear_fd", clear):
                self.prune(apply=True)
            out, _ = processes[0].communicate(timeout=5)
            self.assertEqual(out.strip(), "acquired")
            self.assertEqual(processes[0].returncode, 0)
        finally:
            for process in processes:
                if process.poll() is None:
                    process.kill()
                process.communicate()

    def test_exception_releases_lock_and_surfaces_partial_cleanup(self):
        profile = self.profile()
        with mock.patch.object(
            cache, "clear_fd", side_effect=OSError("injected failure")
        ):
            with self.assertRaisesRegex(cache.CacheError, "partial cleanup"):
                self.prune(apply=True)
        self.assertTrue((profile / "deps/artifact").exists())
        self.assertEqual(
            cache.lock_status(profile / ".cargo-lock"), "available (snapshot only)"
        )

    def test_interrupt_releases_lock(self):
        profile = self.profile()
        with mock.patch.object(cache, "clear_fd", side_effect=KeyboardInterrupt):
            with self.assertRaises(KeyboardInterrupt):
                self.prune(apply=True)
        self.assertEqual(
            cache.lock_status(profile / ".cargo-lock"), "available (snapshot only)"
        )

    def test_external_target_env_rejects_apply(self):
        profile = self.profile()
        with mock.patch.dict(
            os.environ, {"CARGO_TARGET_DIR": str(self.repo / "shared")}
        ):
            with self.assertRaisesRegex(cache.CacheError, "external CARGO_TARGET_DIR"):
                self.prune(apply=True)
        self.assertTrue((profile / "deps/artifact").exists())

    def test_target_symlink_rejected(self):
        real = self.target.with_name("real-target")
        self.target.rename(real)
        self.target.symlink_to(real, target_is_directory=True)
        with self.assertRaisesRegex(cache.CacheError, "symlink"):
            self.prune(apply=True)

    def test_profile_and_triple_symlinks_rejected(self):
        outside = self.repo / "outside"
        outside.mkdir()
        for relative in ("debug", "aarch64-unknown-linux-gnu", "rust-analyzer"):
            link = self.target / relative
            link.symlink_to(outside, target_is_directory=True)
            with self.assertRaisesRegex(cache.CacheError, "symlink"):
                self.prune(apply=True)
            link.unlink()

    def test_internal_symlink_unlinked_without_following(self):
        profile = self.profile()
        outside = self.repo / "outside"
        outside.mkdir()
        (outside / "keep").write_text("keep")
        (profile / "linked").symlink_to(outside, target_is_directory=True)
        self.age(profile, 30)
        self.prune(apply=True)
        self.assertEqual((outside / "keep").read_text(), "keep")
        self.assertFalse((profile / "linked").exists())

    def test_lock_symlink_rejected(self):
        profile = self.profile(lock=False)
        external = self.repo / "lock"
        external.touch()
        (profile / ".cargo-lock").symlink_to(external)
        self.age(profile, 30)
        with self.assertRaises(OSError):
            self.prune(apply=True)
        self.assertTrue((profile / "deps/artifact").exists())

    def test_fifo_lock_rejected_without_blocking(self):
        profile = self.profile(lock=False)
        os.mkfifo(profile / ".cargo-lock")
        self.age(profile, 30)
        with self.assertRaisesRegex(cache.CacheError, "invalid Cargo lock"):
            self.prune(apply=True)
        self.assertTrue((profile / "deps/artifact").exists())

    def test_ancestor_symlink_open_rejected(self):
        original = self.repo / "real"
        original.mkdir()
        (original / "child").mkdir()
        alias = self.repo / "alias"
        alias.symlink_to(original, target_is_directory=True)
        with self.assertRaises(OSError):
            cache.open_directory(alias / "child")

    def test_nested_cargo_lock_rejected(self):
        profile = self.profile()
        (profile / "deps/.cargo-lock").touch()
        self.age(profile, 30)
        with self.assertRaisesRegex(cache.CacheError, "nested Cargo lock"):
            self.prune(apply=True)
        self.assertTrue((profile / "deps/artifact").exists())

    def test_analyzer_requires_separate_selection_and_keeps_root(self):
        normal = self.profile()
        analyzer = self.profile("rust-analyzer/debug")
        self.prune(apply=True)
        self.assertTrue((analyzer / "deps/artifact").exists())
        self.prune(profiles=[], rust_analyzer=True, apply=True)
        self.assertEqual(list(analyzer.iterdir()), [analyzer / ".cargo-lock"])
        self.assertTrue(normal.is_dir())

    def test_cross_target_profile_is_selected(self):
        profile = self.profile("aarch64-unknown-linux-gnu/debug")
        self.prune(apply=True)
        self.assertEqual(list(profile.iterdir()), [profile / ".cargo-lock"])

    def test_allocation_counts_hardlinks_once(self):
        profile = self.profile()
        source = profile / "deps/artifact"
        os.link(source, profile / "deps/hardlink")
        expected = sum(
            path.lstat().st_blocks * 512
            for path in (profile, profile / "deps", source, profile / ".cargo-lock")
        )
        self.assertEqual(cache.measure(profile)["allocated_bytes"], expected)

    def test_sccache_inspection_is_read_only(self):
        shared = self.repo / "sccache"
        shared.mkdir()
        artifact = shared / "entry"
        artifact.write_bytes(b"cache")
        with mock.patch.dict(
            os.environ,
            {
                "SCCACHE_DIR": str(shared),
                "SCCACHE_CACHE_SIZE": "12G",
                "SCCACHE_CONF": str(self.repo / "no-config"),
            },
        ):
            result = cache.sccache_report()
        self.assertEqual(result["configured_limit"], "12G")
        self.assertGreater(result["allocated_bytes"], 0)
        self.assertEqual(artifact.read_bytes(), b"cache")

    def test_cli_error_exit_is_nonzero(self):
        with (
            mock.patch.object(cache, "REPO", self.repo),
            contextlib.redirect_stderr(io.StringIO()) as output,
        ):
            self.assertEqual(cache.main(["prune", "--apply", "--json"]), 1)
        self.assertIn("requires", json.loads(output.getvalue())["error"])


if __name__ == "__main__":
    unittest.main()
