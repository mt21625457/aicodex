## 0. Review and Dependency Gate

- [ ] 0.1 Human review approves this change's `proposal.md`, `design.md`, both
      specs, and `tasks.md` before implementation.
- [ ] 0.2 Confirm `add-cc-style-file-tools` Phase A is approved, implemented,
      and green: default-disabled `dedicated_file_tools` gate, hidden handlers,
      bounded turn receipt with originating sampling-step identity, same-batch
      fail-closed tests, PathUri, shared reviewable mutation, encoding/CRLF,
      commit-time preconditions, local and remote tests.
- [ ] 0.3 Lock `ChatFileToolMode` values and default:
      `legacy` (default), `dedicated`, `dedicated_with_apply_patch`.
- [ ] 0.4 Confirm the locked review decisions: top-level config placement,
      session-invariant mode, staged dependency errors, bounded context-fragment
      format, bounded telemetry enums, and mode truth table.
- [ ] 0.5 Run
      `openspec validate enable-chat-dedicated-file-tools --strict`.

## 1. Config and Planning Policy

- [ ] 1.1 Add typed `ChatFileToolMode`; reject unknown values and do not model
      the policy as multiple booleans. Store it as top-level
      `chat_file_tool_mode` in `ConfigToml`/resolved Config.
- [ ] 1.2 Default to `legacy`; existing Chat request/tool exposure MUST remain
      deep-equal when the field is omitted.
- [ ] 1.3 For a Chat non-legacy mode, reject resolved session config when the
      `dedicated_file_tools` gate is disabled. If the finalized tool plan or
      serialized side metadata lacks any dedicated runtime, declaration, or
      reverse mapping, fail request construction before the first HTTP sampling
      call. Every error names the selected mode, missing dependency, and
      `legacy` rollback; never silently downgrade.
- [ ] 1.4 In `dedicated`, register `read_file` / `edit_file` / `write_file` as
      direct and ApplyPatch as hidden dispatch.
- [ ] 1.5 In `dedicated_with_apply_patch`, advertise all three dedicated tools
      plus ApplyPatch. Keep this mode explicit and non-default.
- [ ] 1.6 Apply this policy only for `WireApi::Chat`; add isolation assertions
      that Responses and Claude model-visible tools remain unchanged.
- [ ] 1.7 Resolve and lock mode when a new session is created. Config changes do
      not switch an existing session at a turn boundary; preserve existing
      conversation history without rewriting prior tool items.

## 2. Chat Wire Names and Guidance

- [ ] 2.1 Reuse `create_tools_json_for_chat_completions`, `chat_tool_name`, and
      `ChatToolCallInfo`; do not special-case dedicated wire names or change the
      hash/sanitization algorithm.
- [ ] 2.2 Assert each mapped dedicated declaration is a Chat function tool with
      the dependency proposal's object schema and stays inside per-tool/total
      Chat context budgets.
- [ ] 2.3 Add `ChatFileToolGuidance` under `core/context` and implement
      `ContextualUserFragment` with fixed markers/template and a hard limit below
      1K tokens. Pass typed resolved mode as non-wire side metadata; after
      serialization, resolve all three first-party semantic identities to their
      actual mapped wire names and render the fragment. Fail request construction
      if non-legacy mode, visible declarations, and reverse metadata disagree.
- [ ] 2.4 Guidance MUST use the callable mapped names, require dependent
      Read→Edit/Write calls across completions, and preserve the dependency's
      binary/encoding/editable-cap shell fallback.
- [ ] 2.5 Do not inject dedicated guidance in `legacy` or when all three tools
      are not model-visible. Third-party/dynamic tools with the same semantic
      names MUST NOT cause legacy guidance injection.
- [ ] 2.6 Keep the rendered fragment deep-equal across retries and later sampling
      steps in one session, and do not append duplicate copies to conversation
      history.

## 3. Chat Request and History Tests

- [ ] 3.1 Deep-equal complete `legacy`, `dedicated`, and
      `dedicated_with_apply_patch` request objects, including `tools`,
      `tool_choice`, `parallel_tool_calls`, system guidance, and side metadata.
- [ ] 3.2 Assert tool reordering and ApplyPatch visibility do not change the
      mapped names of dedicated tools.
- [ ] 3.3 Assert mapped Chat names reverse to semantic
      `read_file` / `edit_file` / `write_file` with
      `ChatToolCallKind::Function`.
- [ ] 3.4 Assert prior hidden ApplyPatch assistant calls serialize with their
      deterministic historical Chat name and matching result `tool_call_id`,
      while ApplyPatch is absent from current `dedicated` declarations.
- [ ] 3.5 Assert a provider/schema/tool-budget construction error is actionable
      and does not trigger an automatic legacy request.
- [ ] 3.6 Assert unknown mode fails config loading; Chat non-legacy with a disabled
      gate fails resolved session config; missing runtime/declaration/mapping
      fails before HTTP; and the same non-legacy field on Responses/Claude does
      not change tools or trigger the Chat gate error.
- [ ] 3.7 Assert guidance stays below its hard cap, remains deep-equal across
      retries/steps, and is absent in legacy even when same-named dynamic tools
      are present.

## 4. Mock End-to-End Chat Tool Loops

- [ ] 4.1 Stream fragmented mapped `read_file` arguments, reconstruct the
      semantic function call, execute locally, and assert the continuation uses
      assistant `tool_calls` plus a `tool` role result with the same call id.
- [ ] 4.2 Execute `read_file -> edit_file -> edit_file` across Chat sampling
      steps; assert first and second edits succeed and the committed file matches.
- [ ] 4.3 Execute `write_file(create) -> edit_file`; assert create establishes the
      dependency proposal's full receipt and immediate refinement succeeds.
- [ ] 4.4 Execute missing/stale receipt cases; assert failure is returned as a
      model-correctable Chat tool result, the file is unchanged, and the turn can
      continue after a new read.
- [ ] 4.5 Exercise multiple tool calls and assert dependent calls cannot use a
      receipt created later/in the same unordered batch to bypass read-before-
      write. Assert the receipt's sampling-step provenance drives this result,
      not scheduler order; independent calls retain existing parallel behavior.
- [ ] 4.6 Verify hidden dispatch can complete a valid resumed legacy call without
      advertising the hidden tool in the next request.

## 5. Remote, Docs, and Rollback

- [ ] 5.1 Build core integrations with
      `TestCodexBuilder::build_with_auto_env()` so Chat tool loops run locally and
      against remote executors.
- [ ] 5.2 Run targeted Linux remote tests via `scripts/test-remote-env.sh` with
      documented cleanup.
- [ ] 5.3 Run Windows/Wine coverage:
      `bazel test //codex-rs/core:core-all-wine-exec-test`.
- [ ] 5.4 Update `docs/config.md` with mode truth table, hard dependency,
      provider compatibility caveat, mapped-name/context-fragment behavior,
      session scope, explicit staged errors, and rollback to `legacy`.
- [ ] 5.5 Run `cd codex-rs && just write-config-schema` and include
      `codex-rs/core/config.schema.json` when config shape changes.
- [ ] 5.6 Test that switching a new session back to `legacy` restores the prior
      Chat declarations without removing shared hidden handlers or rewriting old
      history.

## 6. Quality Gates

- [ ] 6.1 During implementation run `cd codex-rs && just test -p codex-core`.
- [ ] 6.2 Run `cd codex-rs && just test -p codex-tools` only if that crate changes.
- [ ] 6.3 Ask before running the complete `cd codex-rs && just test` suite if
      core/common/protocol changes require it.
- [ ] 6.4 Before finalizing, run scoped `cd codex-rs && just fix -p <project>`,
      then `cd codex-rs && just fmt`; do not rerun tests after final fix/fmt.
- [ ] 6.5 Run
      `openspec validate enable-chat-dedicated-file-tools --strict`.
- [ ] 6.6 Keep the implementation below repository change-size guidance. If Chat
      SSE parsing or filesystem semantics need modification, stop and update the
      owning dependency change rather than expanding this rollout PR.
