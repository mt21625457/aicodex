## 0. Review Gate

- [ ] 0.1 Human review approves `proposal.md`, `design.md`, and all three specs
      before any Rust implementation begins.
- [ ] 0.2 Confirm the finite policy truth table:
      - rollout gate off restores the existing tool plan;
      - `auto` uses native `text_editor` for supported Anthropic models and
        dedicated tools for Compatible providers and Kimi K3;
      - `dedicated` opts Anthropic into dedicated tools;
      - only `dedicated_with_apply_patch` advertises both dedicated tools and
        `apply_patch`;
      - Responses remains on `apply_patch` in this change.
- [ ] 0.3 Lock schema field name (`path` by default), 1-based `offset`/`limit`,
      multi-environment `environment_id`, result metadata, output token/byte
      caps, editable byte cap, and receipt-store hard limits.
- [ ] 0.4 Confirm unsupported text/encoding behavior and the minimum supported
      round-trip matrix: UTF-8, UTF-8 BOM, LF, and CRLF.
- [ ] 0.5 Run `openspec validate add-cc-style-file-tools --strict`.

## 1. Phase A — Hidden Safety Foundation

- [ ] 1.1 Add a focused `file_tools/` module under
      `codex-rs/core/src/tools/handlers/` with separate schema/handler/receipt
      modules and sibling `*_tests.rs` files. Keep implementation modules below
      the repository size guidance and avoid growing central orchestration files.
- [ ] 1.2 Resolve model path strings against the selected environment's
      `cwd: PathUri`; use executor canonicalization for existing targets and
      canonicalized parents for creates. Key receipts by
      `(environment_id, canonical PathUri)` and never convert foreign paths to
      host-native `PathBuf` for authorization or identity.
- [ ] 1.3 Implement a bounded, turn-scoped receipt store containing fixed-size
      fingerprints, file size/mtime, merged observed line ranges, full-coverage,
      and write-eligibility. Add hard limits for entries, ranges, and memory.
- [ ] 1.4 Implement `read_file` through `ExecutorFileSystem` metadata/stream APIs
      with 1-based line-numbered output, optional offset/limit, total/range/
      completeness metadata, hard output caps, and explicit write-eligibility.
      Range reads below the editable cap compute a full-file fingerprint without
      injecting unbounded content into model context.
- [ ] 1.5 Extract or reuse a shared round-trip text representation used by both
      apply-patch mutations and dedicated tools. Preserve UTF-8 BOM and CRLF for
      edits; reject binary, device, UTF-16, or other non-round-trippable text with
      a model-correctable error rather than silently transcoding.
- [ ] 1.6 Implement `edit_file` exact replacement + `replace_all`. Require every
      affected occurrence to fall inside observed receipt ranges, reject
      create-missing-file, compare the current fingerprint on every attempt, and
      refresh/invalidate the receipt after completion.
- [ ] 1.7 Implement `write_file` create and overwrite. Require full receipt
      coverage for overwrite; require a commit-time no-clobber precondition for
      create; refresh/invalidate the receipt after completion.
- [ ] 1.8 Extract a shared reviewable file-mutation path used by ApplyPatch,
      native text editor, and dedicated writes. Preserve outer tool identities
      for hooks/telemetry/model results while reusing approval, sandbox,
      file-change/diff events, and commit-time expected-content checks.
- [ ] 1.9 Register all three new handlers as hidden dispatch only. Phase A MUST
      NOT change the model-visible tool list or inject dedicated-tool guidance.

## 2. Phase A Verification

- [ ] 2.1 Unit-test schemas, 1-based ranges, store bounds/eviction, PathUri
      resolution, environment-separated keys, unique/`replace_all` behavior,
      observed-range coverage, fingerprints, and create no-clobber checks.
- [ ] 2.2 Test both `read_file -> edit_file -> edit_file` and
      `write_file(create) -> edit_file` in one user turn to prove every
      successful mutation refreshes or establishes the receipt.
- [ ] 2.3 Test missing receipt, partial-read range violations, partial-read
      overwrite rejection, same-mtime changed content, and changed-mtime
      unchanged content without mutating on failure.
- [ ] 2.4 Test an external modification while approval is pending; commit MUST
      reject the stale mutation.
- [ ] 2.5 Test UTF-8 BOM/CRLF preservation and safe rejection of unsupported
      encoding/binary inputs.
- [ ] 2.6 Add core integration coverage with
      `TestCodexBuilder::build_with_auto_env()` for local/remote filesystem
      behavior and foreign Windows paths. Add a multi-environment case proving
      identical path text does not share receipts across environments.
- [ ] 2.7 If new tool lifecycle text or names render in the TUI, add/update and
      review the required `insta` snapshots.

## 3. Phase B — Compatible and Kimi K3 Auto Rollout

- [ ] 3.1 Add the `dedicated_file_tools` rollout feature and typed
      `ClaudeFileToolMode` enum (`auto`, `dedicated`,
      `dedicated_with_apply_patch`). Reject unknown values and avoid multiple
      interdependent boolean flags.
- [ ] 3.2 In `auto`, advertise dedicated tools for Claude Compatible providers
      and models recognized as Kimi K3. Keep ApplyPatch registered as hidden
      dispatch and do not advertise Anthropic-native `text_editor_*` types.
- [ ] 3.3 In Anthropic `auto`, advertise the supported native `text_editor` as
      the sole primary editor, keep ApplyPatch hidden, and decouple native editor
      capability from ApplyPatch model visibility.
- [ ] 3.4 Add `dedicated` and `dedicated_with_apply_patch` Anthropic opt-in modes
      exactly as specified by the truth table. Keep hidden handlers callable for
      valid resumed legacy transcript calls.
- [ ] 3.5 Keep OpenAI Responses and Chat wire behavior unchanged; this proposal
      MUST NOT advertise dedicated file tools there.
- [ ] 3.6 Generate model-facing prefer-dedicated guidance only from the actual
      visible tool set. Update both `exec_command` and `shell_command` specs;
      avoid relying only on core-host `cfg!(windows)` for remote Windows wording.
      Document the explicit large/binary/unsupported-text shell fallback.
- [ ] 3.7 Update `docs/config.md` with the rollout gate, mode truth table,
      remote-path semantics, safety limits, and rollback behavior.
- [ ] 3.8 If config types or features schema change, run
      `cd codex-rs && just write-config-schema` and include the result.

## 4. Phase B Request and Tool-Loop Verification

- [ ] 4.1 Assert the complete request truth table for rollout-off, Anthropic
      `auto`, Anthropic `dedicated`, Anthropic fallback, Compatible `auto`,
      Compatible fallback, and Kimi K3 `auto`. The K3 request MUST contain the
      three dedicated JSON function tools and no native `text_editor_*` or
      `apply_patch`. Assertions cover visible tools and hidden registry exposure.
- [ ] 4.2 Claude mock integration: Compatible `read_file` then `edit_file`
      succeeds and the next request contains a non-error `tool_result`.
- [ ] 4.3 Claude mock integration: missing/stale receipt fails without disk
      mutation and returns a model-correctable read-again result.
- [ ] 4.4 Resume integration: hidden ApplyPatch/native handlers execute a valid
      legacy call but remain absent from the new model-visible request.
- [ ] 4.5 Assert dynamic guidance is present only when all dedicated tools are
      visible and applies with either `exec_command` or `shell_command`. Do not
      add tests that only restate a static description constant.
- [ ] 4.6 Run remote executor verification:
      - Linux: source `scripts/test-remote-env.sh` and run the targeted
        `codex-core` integration tests with cleanup as documented by
        `$remote-tests`;
      - Windows/Wine: `bazel test //codex-rs/core:core-all-wine-exec-test`.

## 5. Final Quality Gates

- [ ] 5.1 During implementation, run targeted tests for every changed crate:
      - `cd codex-rs && just test -p codex-core`;
      - `cd codex-rs && just test -p codex-tools` only if `codex-tools` changed;
      - `cd codex-rs && just test -p codex-apply-patch` only if that crate
        changed.
- [ ] 5.2 Ask before running the complete `cd codex-rs && just test` suite if
      common/core/protocol changes require it.
- [ ] 5.3 Before finalizing each large Rust phase, run the appropriately scoped
      `cd codex-rs && just fix -p <project>`, then `cd codex-rs && just fmt`.
      Do not rerun tests after the final fix/fmt pass.
- [ ] 5.4 Run `openspec validate add-cc-style-file-tools --strict` and review the
      actual diff against the phase's model-visibility invariant.
- [ ] 5.5 Keep each PR below the repository change-size guidance. If Phase A or B
      exceeds it, split only along hidden internal module boundaries; never land
      a direct/model-visible tool without its receipt, mutation precondition,
      mutual-exclusion, prompt, and integration-test requirements.

## 6. Phase C — Anthropic Experiment and Default Decision

- [ ] 6.1 Roll out Anthropic `dedicated` only as explicit opt-in and collect
      bounded telemetry for tool selection, shell fallback, edit success,
      stale-retry, approval rejection, and unsupported-file fallback.
- [ ] 6.2 Verify disabling `dedicated_file_tools` restores the previous request
      tool plan without removing hidden compatibility handlers.
- [ ] 6.3 Any proposal to make Anthropic dedicated mode the default or add
      `Read`/`Edit`/`Write` aliases MUST use observed rollout data and a new
      OpenSpec change.
