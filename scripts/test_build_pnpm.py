"""Windows migration from virtual-store symlinks to pnpm's hoisted layout."""

import subprocess
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch

from scripts.test_build import build


class PnpmLinksTest(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name).resolve()
        self.modules = self.root / "sdk/typescript/node_modules"
        self.modules.mkdir(parents=True)
        self.store = self.root / "node_modules/.pnpm"
        self.store.mkdir(parents=True)
        for name, value in (("REPO_ROOT", self.root),):
            p = patch.object(build, name, value)
            p.start()
            self.addCleanup(p.stop)

    def link(self, path, target):
        path.parent.mkdir(parents=True, exist_ok=True)
        path.symlink_to(target, target_is_directory=True)

    def repair(self, linker="hoisted"):
        with (
            patch.object(build.sys, "platform", "win32"),
            patch.object(build, "run", return_value=subprocess.CompletedProcess(
                [], 0, stdout=linker + "\n"
            )),
        ):
            build.remove_legacy_pnpm_links()

    def test_removes_only_store_links_including_scoped_and_dangling(self):
        target = self.store / "prettier/node_modules/prettier"
        target.mkdir(parents=True)
        marker = target / "keep"
        marker.write_text("preserved")
        link = self.modules / "prettier"
        self.link(link, target)
        dangling = self.modules / "@types/missing"
        self.link(dangling, self.store / "missing")
        external = self.root / "external"
        external.mkdir()
        outside = self.modules / "external"
        self.link(outside, external)
        real = self.modules / "real"
        real.mkdir()
        self.repair()
        self.assertFalse(link.is_symlink())
        self.assertFalse(dangling.is_symlink())
        self.assertTrue(outside.is_symlink())
        self.assertTrue(real.is_dir())
        self.assertEqual(marker.read_text(), "preserved")
        self.repair()  # Idempotent.

    def test_isolated_layout_is_untouched(self):
        link = self.modules / "prettier"
        self.link(link, self.store / "missing")
        self.repair("isolated")
        self.assertTrue(link.is_symlink())

    def test_interrupted_migration_cleans_old_shims_only(self):
        bins = self.modules / ".bin"
        bins.mkdir()
        for name in ("tsup", "tsup.cmd", "tsup.ps1"):
            (bins / name).write_text('NODE_PATH="../node_modules/.pnpm/tsup"')
        (bins / "custom.cmd").write_text("echo keep")
        (bins / "new.cmd").write_text('NODE_PATH="../node_modules/tsup"')
        self.repair()
        self.assertEqual(
            sorted(p.name for p in bins.iterdir()), ["custom.cmd", "new.cmd"]
        )

    def test_redirected_scope_is_not_traversed(self):
        outside = self.root / "outside"
        outside.mkdir()
        link = outside / "package"
        self.link(link, self.store / "missing")
        self.link(self.modules / "@scope", outside)
        self.repair()
        self.assertTrue(link.is_symlink())

    def test_redirected_modules_is_not_traversed(self):
        self.modules.rmdir()
        outside = self.root / "outside"
        outside.mkdir()
        link = outside / "package"
        self.link(link, self.store / "missing")
        self.link(self.modules, outside)
        self.repair()
        self.assertTrue(link.is_symlink())

    def test_non_windows_does_not_query_or_clean(self):
        with (
            patch.object(build.sys, "platform", "linux"),
            patch.object(build, "run") as run,
        ):
            build.remove_legacy_pnpm_links()
        run.assert_not_called()

    def test_install_repairs_before_running_pnpm(self):
        calls = []
        with (
            patch.object(build, "node_available", return_value=True),
            patch.object(build, "pnpm_available", return_value=True),
            patch.object(build, "remove_legacy_pnpm_links",
                         side_effect=lambda: calls.append("repair")),
            patch.object(build, "run",
                         side_effect=lambda *a, **kw: calls.append(a[0])),
        ):
            build.build_ts()
        self.assertEqual(calls, ["repair", ["pnpm", "install"]])

    def test_skip_install_does_not_clean_links(self):
        with (
            patch.object(build, "node_available", return_value=True),
            patch.object(build, "pnpm_available", return_value=True),
            patch.object(build, "remove_legacy_pnpm_links") as repair,
            patch.object(build, "run") as run,
        ):
            build.build_ts(install_deps=False)
        repair.assert_not_called()
        run.assert_not_called()


if __name__ == "__main__":
    unittest.main()
