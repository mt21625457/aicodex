## 0. Review Gate

- [ ] 0.1 Human review approves `proposal.md`, `design.md`, and all three specs
      before any Rust implementation begins.
- [ ] 0.2 Confirm the finite policy truth table:
      - rollout gate off restores the existing tool plan;
      - rollout gate off does not register the three dedicated handler names;
      - `auto` uses native `text_editor` for supported Anthropic models and
        dedicated tools for Compatible providers and Kimi K3;
      - `dedicated` opts Anthropic into dedicated tools;
      - only `dedicated_with_apply_patch` advertises both dedicated tools and
        `apply_patch`;
      - Responses and Chat Completions remain on their existing tool plans.
- [ ] 0.3 Confirm the locked schema/limits from design §5: `path`, 1-based
      `offset <= 1,000,000`, `limit <= 2,000`, optional multi-environment
      `environment_id`, fixed result header, `64 KiB / ~10,000 token` output and
      mutation-argument caps, `8 MiB` editable cap, `64 MiB` scan cap, and
      `128 entries / 64 ranges per entry / 1,024 total ranges / 256 KiB`
      receipt-store caps.
- [ ] 0.4 Confirm raw-byte SHA-256 receipt identity and the shared round-trip
      encoding contract: UTF-8/BOM and existing lossless legacy encodings stay
      supported; UTF-16, binary, and non-round-trippable text reject.
- [x] 0.5 Run `openspec validate add-cc-style-file-tools --strict`.

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
      raw-byte SHA-256, file size/mtime, merged observed line ranges, full-coverage,
      write-eligibility, and the originating sampling-step identity. Add hard
      limits exactly as locked in design §5. Edit/Write MUST reject receipts
      created or refreshed by another call in the same provider response/batch.
- [ ] 1.4 Extend `codex-file-system` with `write_file_conditional` and typed writes:
      `MustNotExist` and `MatchSha256([u8; 32])`, plus a distinct conflict
      result. `MustNotExist` MUST use atomic create-new semantics; precondition
      support MUST fail closed rather than fall back to unconditional write.
- [ ] 1.5 Add internal exec-server RPC `fs/conditionalWriteFile` and extend
      `codex-exec-server-protocol` / `codex-exec-server` so remote
      and sandboxed filesystems support bounded streaming reads and executor-side
      conditional commits in one RPC/operation. Old servers that lack the new
      method MUST return an unsupported/correctable failure, never an
      unconditional compatibility write. Do not change app-server's public v2
      `fs/writeFile` API for this internal executor capability.
- [ ] 1.6 Implement `read_file` through `ExecutorFileSystem` metadata/stream APIs
      with the fixed header, 1-based line-numbered output, optional offset/limit,
      nullable total-line metadata, completeness/write-eligibility, and all
      design §5 caps. Range reads below `8 MiB` compute full raw SHA-256; larger
      range reads stop at `64 MiB` and report total lines unknown unless EOF was
      reached.
- [ ] 1.7 Extract or reuse a shared round-trip text representation used by both
      apply-patch mutations and dedicated tools. Preserve UTF-8 BOM and CRLF for
      edits and retain all encodings the existing shared decoder can losslessly
      round-trip; reject binary, device, UTF-16, or other non-round-trippable text.
- [ ] 1.8 Implement `edit_file` exact replacement + `replace_all`. Require every
      affected occurrence to fall inside observed receipt ranges, reject
      create-missing-file, compare raw SHA-256 on every attempt, and
      refresh/invalidate the receipt after completion.
- [ ] 1.9 Implement `write_file` create and overwrite. Require full receipt
      coverage + `MatchSha256` for overwrite and atomic `MustNotExist` for create;
      refresh/invalidate the receipt after completion.
- [ ] 1.10 Extract a shared reviewable file-mutation path used by ApplyPatch,
      native text editor, and dedicated writes. Preserve outer tool identities
      for hooks/telemetry/model results while reusing approval, sandbox,
      file-change/diff events, keyed mutation serialization, and executor-side
      precondition checks.
- [ ] 1.11 Add the default-disabled typed feature config
      `[features.dedicated_file_tools]` with `enabled` and `mode` (`auto` by
      default). Gate off MUST omit all three handlers from the registry. In
      Phase A, explicitly enabling the gate registers them as hidden dispatch
      only and MUST NOT inject dedicated-tool guidance.

## 2. Phase A Verification

- [ ] 2.1 Unit-test exact schemas, fixed output header, every design §5 boundary,
      store LRU/range invalidation, PathUri resolution, environment-separated
      keys, unique/`replace_all` behavior, observed-range coverage, raw SHA-256,
      and conflict error mapping.
- [ ] 2.2 Test both `read_file -> edit_file -> edit_file` and
      `write_file(create) -> edit_file` in one user turn to prove every
      successful mutation refreshes or establishes the receipt.
- [ ] 2.3 Test missing receipt, partial-read range violations, partial-read
      overwrite rejection, same-mtime changed content, and changed-mtime
      unchanged content without mutating on failure.
- [ ] 2.4 Test a Read/Edit pair and a create/Edit pair emitted in one unordered
      provider tool-call batch. Even if the Read or create executes first, its
      same-step receipt MUST NOT authorize the dependent mutation; the file must
      remain unchanged except for an independently valid create.
- [ ] 2.5 Test two overwrite mutations emitted in one batch; they MUST snapshot
      the prior-step receipt and MUST NOT let the first completion refresh state
      that silently authorizes the second.
- [ ] 2.6 Test an external modification while approval is pending; executor-side
      `MatchSha256` MUST reject the stale mutation. Test a concurrently appeared
      create target; atomic `MustNotExist` MUST leave it unchanged.
- [ ] 2.7 Test local, sandboxed, remote Linux, and Wine/Windows conditional writes.
      Test sandbox-compatible stream reads, `64 MiB` scan termination, and
      fail-closed behavior against an exec-server without conditional-write
      support.
- [ ] 2.8 Test UTF-8 BOM/CRLF and a representative existing legacy encoding
      round trip, plus safe rejection of UTF-16/unsupported/binary inputs.
- [ ] 2.9 Add core integration coverage with
      `TestCodexBuilder::build_with_auto_env()` for local/remote filesystem
      behavior and foreign Windows paths. Add a multi-environment case proving
      identical path text does not share receipts across environments.
- [ ] 2.10 Assert gate-off excludes `read_file`/`edit_file`/`write_file` from both
      model-visible specs and the dispatch registry; a forged direct call returns
      unsupported-tool without touching the filesystem.
- [ ] 2.11 If new tool lifecycle text or names render in the TUI, add/update and
      review the required `insta` snapshots.

## 3. Phase B — Compatible and Kimi K3 Auto Rollout

- [ ] 3.1 Complete the typed `[features.dedicated_file_tools]` config with
      `ClaudeFileToolMode` (`auto`, `dedicated`,
      `dedicated_with_apply_patch`), boolean shorthand compatibility, object
      default `mode=auto`, and deny-unknown-fields/value tests. Avoid multiple
      interdependent boolean flags.
- [ ] 3.2 In `auto`, advertise dedicated tools for Claude Compatible providers
      and models recognized as Kimi K3. Keep ApplyPatch registered as hidden
      dispatch and do not advertise Anthropic-native `text_editor_*` types.
- [ ] 3.3 In Anthropic `auto`, advertise the supported native `text_editor` as
      the sole primary editor, keep ApplyPatch hidden, and decouple native editor
      capability from ApplyPatch model visibility.
- [ ] 3.4 Add `dedicated` and `dedicated_with_apply_patch` Anthropic opt-in modes
      exactly as specified by the truth table. Keep only the pre-existing hidden
      ApplyPatch/native handlers callable for valid resumed legacy transcript
      calls; this must not weaken the dedicated gate-off registry invariant.
- [ ] 3.5 Keep OpenAI Responses and Chat wire behavior unchanged; this proposal
      MUST NOT advertise dedicated file tools there.
- [ ] 3.6 Generate model-facing prefer-dedicated guidance only from the actual
      visible tool set. Update both `exec_command` and `shell_command` specs;
      avoid relying only on core-host `cfg!(windows)` for remote Windows wording.
      Document the explicit large/binary/unsupported-text shell fallback.
- [ ] 3.7 Update `docs/config.md` with the rollout gate, mode truth table,
      remote-path semantics, safety limits, and rollback behavior.
- [ ] 3.8 Run `cd codex-rs && just write-config-schema` and include the updated
      config schema fixture.

## 4. Phase B Request and Tool-Loop Verification

- [ ] 4.1 Assert the complete request/registry truth table for rollout-off,
      Responses, Chat Completions, Anthropic
      `auto`, Anthropic `dedicated`, Anthropic fallback, Compatible `auto`,
      Compatible fallback, and Kimi K3 `auto`. The K3 request MUST contain the
      three dedicated JSON function tools and no native `text_editor_*` or
      `apply_patch`. Rollout-off, Responses, and Chat MUST not register the three
      dedicated names because of this change.
- [ ] 4.2 Claude mock integration: Compatible `read_file` then `edit_file`
      succeeds and the next request contains a non-error `tool_result`.
- [ ] 4.3 Claude mock integration: missing/stale receipt fails without disk
      mutation and returns a model-correctable read-again result.
- [ ] 4.4 Resume integration: existing hidden ApplyPatch/native handlers execute
      a valid legacy call but remain absent from the new model-visible request.
      This compatibility rule MUST NOT keep the three new dedicated handlers
      registered when their gate is off.
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
      - `cd codex-rs && just test -p codex-file-system`;
      - `cd codex-rs && just test -p codex-exec-server-protocol`;
      - `cd codex-rs && just test -p codex-exec-server`;
      - `cd codex-rs && just test -p codex-features`;
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
- [ ] 5.5 If any Rust dependency manifest changes, run repository-root
      `just bazel-lock-update` and include `MODULE.bazel.lock`.
- [ ] 5.6 Keep each PR below the repository change-size guidance. If Phase A or B
      exceeds it, split only along hidden internal module boundaries; never land
      a direct/model-visible tool without its receipt, mutation precondition,
      mutual-exclusion, prompt, and integration-test requirements.

## 6. Phase C — Anthropic Experiment and Default Decision

- [ ] 6.1 Roll out Anthropic `dedicated` only as explicit opt-in and collect
      bounded telemetry for tool selection, shell fallback, edit success,
      stale-retry, approval rejection, and unsupported-file fallback.
- [ ] 6.2 Verify disabling `dedicated_file_tools` restores the previous request
      and registry plan: dedicated handlers disappear, while pre-existing hidden
      ApplyPatch/native compatibility handlers retain their existing behavior.
- [ ] 6.3 Any proposal to make Anthropic dedicated mode the default or add
      `Read`/`Edit`/`Write` aliases MUST use observed rollout data and a new
      OpenSpec change.
