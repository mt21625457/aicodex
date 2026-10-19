# Local build storage

Routine development uses Rust 1.96.0 from both the repository root and
`codex-rs/`. Keep the two `rust-toolchain.toml` files in sync when upgrading.
The root Cargo configuration disables incremental compilation by default.
The `dev`, `test`, and compatibility `dev-small` profiles disable debug information
and split debug sidecars. Release and profiling symbol policies are unchanged.

These defaults reduce retained compiler output; they do not impose a size limit
on `target/`. Changing the compiler, features, profiles, flags, or target can still
produce additional dependency variants. Existing artifacts are not removed when
the configuration changes, and the first build with new settings may temporarily
increase usage.

## Consistent development commands

From the repository root, with Python 3.11 or newer:

```sh
python3 scripts/dev_cargo.py check --dry-run
just dev-check
just dev-build
just dev-test -- --lib
```

The three commands always run from `codex-rs/`, use `--locked`, and select the same
runtime package group: `codex-core`, `codex-app-server`, `codex-cli`,
`codex-code-mode-host`, and `codex-rmcp-client`. Check defaults to `--all-targets`.
Tests use cargo-nextest, the `local` nextest profile, and a 32 MiB minimum Rust
thread stack. Install cargo-nextest before running tests. Existing `just` recipes
remain available.

Use `--group workspace` before the argument separator to select the whole
workspace consistently. Pass test and target filters after `--`, for example:

```sh
just dev-test --group workspace -- --lib -E 'test(session)'
just dev-build --profile dev-debug
```

`dev-debug` provides full debugger information, packed debug sidecars (including
macOS dSYM files), and its own output directory. Prefer this profile for debugging.
For direct Cargo commands, pair `CARGO_PROFILE_DEV_DEBUG=full` with
`CARGO_PROFILE_DEV_SPLIT_DEBUGINFO=packed`; use the corresponding
`CARGO_PROFILE_TEST_*` variables for tests. Overriding only `DEBUG` is insufficient
to retain usable macOS debugging information with the default split setting.
Debug information increases storage. `CARGO_INCREMENTAL=1` explicitly re-enables
incremental compilation.

The wrapper is for the default host target. It rejects `CARGO_BUILD_TARGET` and
Cargo configuration files declaring `build.target`, including an explicit host
triple, to avoid silently creating another target directory. Use Cargo directly
for custom targets, features, package combinations, or release builds. The wrapper
resolves V8 through the existing official artifact helper before execution;
`--dry-run` only prints the command and does not download artifacts or run Cargo.
Check, build, and test can still require distinct artifacts even with a fixed
package selection.

## Inspect and prune old build artifacts

```sh
just build-cache
python3 scripts/build_cache.py report --json
python3 scripts/build_cache.py prune --profile debug --rust-analyzer
```

The tool manages only this checkout's standard `codex-rs/target` directory. It
does not follow custom Cargo target-directory configuration. Reports include
allocated bytes, known profile directories, rust-analyzer usage, and a read-only
snapshot of the shared sccache directory. Hard links are counted once within each
row and across the total; rows may overlap and are not guaranteed reclaim sizes.
The tool currently requires macOS or Linux filesystem accounting and locking.

Pruning defaults to a preview and requires an explicit `--profile` and/or
`--rust-analyzer` selection. Only entire selected profiles whose newest entry is
at least 14 days old qualify. A recent entry retains the entire profile. This is
not per-dependency garbage collection or an automatic scheduled cleanup.
`--profile debug` includes that profile under cross-compilation target triples;
rust-analyzer profiles require the separate `--rust-analyzer` selection.

After reviewing the preview, append `--apply` to the same command to delete the
eligible contents. To deliberately clear recent artifacts, first preview with
`--min-age-days 0`, then append `--apply`. Cleared artifacts must be rebuilt.
The tool preserves profile directories and their original `.cargo-lock` files,
acquires Cargo-compatible locks before deleting, and rechecks age while locked.
A busy selected candidate aborts cleanup before deletion. Symlinked target paths
and an external `CARGO_TARGET_DIR` are rejected for cleanup; internal symlinks are
unlinked without following them. Filesystem errors or interruption can leave a
partial cleanup; rerun the report afterward. Do not concurrently move target
directories or run another non-Cargo cleaner.

## Shared sccache budget

The repository supplies `SCCACHE_CACHE_SIZE=10G` to Cargo child processes unless
the caller already set it. This affects a new sccache daemon started with that
environment. It does not change an already running shared daemon, force existing
cache data below 10 GiB immediately, or enable sccache if no wrapper is configured.
The cache tool never stops the daemon or deletes its shared cache.

The report's `configured_limit` comes from the calling environment or sccache's
own config file, not Cargo's `[env]` defaults. It is not the live daemon limit.
Other projects and daemons started outside this checkout can use other budgets.
Apply any shared-daemon lifecycle change only when its users are idle.

## Validate changes to this policy

```sh
python3 -m unittest scripts.test_build_storage_config scripts.test_dev_cargo scripts.test_build_cache
```

Cleanup tests operate only on temporary directories. They cover dry-run behavior,
age selection, active locks, waiting processes, preserved lock inodes, symlinks,
and error handling; they do not clear the real repository cache.
