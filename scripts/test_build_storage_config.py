"""Validate the actual Cargo storage defaults without building the workspace."""

from pathlib import Path
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]


def read_toml(path):
    with path.open("rb") as source:
        return tomllib.load(source)


class BuildStorageConfigTest(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.config = read_toml(ROOT / ".cargo/config.toml")
        cls.profiles = read_toml(ROOT / "codex-rs/Cargo.toml")["profile"]

    def test_root_and_nested_toolchains_match(self):
        root = read_toml(ROOT / "rust-toolchain.toml")
        nested = read_toml(ROOT / "codex-rs/rust-toolchain.toml")
        self.assertEqual(root, nested)
        self.assertEqual(root["toolchain"]["channel"], "1.96.0")
        self.assertEqual(
            set(root["toolchain"]["components"]), {"clippy", "rustfmt", "rust-src"}
        )

    def test_incremental_is_disabled_without_forcing_the_environment(self):
        self.assertIs(self.config["build"]["incremental"], False)
        self.assertNotIn("CARGO_INCREMENTAL", self.config.get("env", {}))
        for name, profile in self.profiles.items():
            with self.subTest(profile=name):
                self.assertIsNot(profile.get("incremental"), True)

    def test_sccache_budget_preserves_explicit_user_environment(self):
        budget = self.config["env"]["SCCACHE_CACHE_SIZE"]
        self.assertEqual(budget["value"], "10G")
        self.assertIs(budget["force"], False)
        # Adding the cache budget must retain the existing allocator policy.
        self.assertEqual(
            self.config["env"]["MALLOC_CONF"],
            {"value": "thp:always", "force": False},
        )

    def test_nested_cargo_config_does_not_override_storage_defaults(self):
        nested = read_toml(ROOT / "codex-rs/.cargo/config.toml")
        self.assertNotIn("incremental", nested.get("build", {}))
        for config in (self.config, nested):
            # Keep profile policy in the workspace manifest for both entrypoints.
            for profile in config.get("profile", {}).values():
                for key in ("debug", "split-debuginfo", "strip", "incremental"):
                    self.assertNotIn(key, profile)
            self.assertNotIn("CARGO_INCREMENTAL", config.get("env", {}))

    def test_default_builds_do_not_emit_debug_info_or_sidecars(self):
        for name in ("dev", "test", "dev-small"):
            with self.subTest(profile=name):
                profile = self.profiles[name]
                self.assertEqual(profile["debug"], "none")
                self.assertEqual(profile["split-debuginfo"], "off")
                # Stripping would silently discard an explicit debug override.
                self.assertIn(profile.get("strip", False), (False, "none"))
        self.assertEqual(self.profiles["dev-small"]["inherits"], "dev")
        self.assertEqual(self.profiles["dev-small"]["opt-level"], 0)

    def test_full_debugging_is_explicitly_available(self):
        profile = self.profiles["dev-debug"]
        self.assertEqual(profile["inherits"], "dev")
        self.assertEqual(profile["debug"], "full")
        self.assertEqual(profile["split-debuginfo"], "packed")
        self.assertIn(profile.get("strip", False), (False, "none"))
        for name in ("CARGO_PROFILE_DEV_DEBUG", "CARGO_PROFILE_TEST_DEBUG"):
            self.assertNotIn(name, self.config.get("env", {}))

    def test_release_and_profiling_keep_symbolication(self):
        release = self.profiles["release"]
        self.assertEqual(release["lto"], "thin")
        self.assertEqual(release["debug"], "line-tables-only")
        self.assertEqual(release["split-debuginfo"], "off")
        self.assertIs(release["strip"], False)
        self.assertEqual(release["codegen-units"], 4)
        profiling = self.profiles["profiling"]
        self.assertEqual(profiling["inherits"], "release")
        self.assertEqual(profiling["debug"], "full")
        self.assertIs(profiling["lto"], False)
        self.assertIs(profiling["strip"], False)
        self.assertEqual(self.profiles["ci-test"]["inherits"], "test")


if __name__ == "__main__":
    unittest.main()
