## 1. OpenSpec and Scope

- [x] 1.1 Validate this change against the existing Claude proposals and keep it
  scoped to conformance hardening, not new public Claude features.
- [x] 1.2 Confirm no OpenAI Responses request or stream parser changes are
  required except provider-neutral helpers.
- [x] 1.3 Run `openspec validate harden-claude-protocol-conformance --strict`.

## 2. Ordered Claude Stream Replay

- [x] 2.1 Refactor the Claude SSE accumulator to finalize content blocks in
  Claude content-block index order.
- [x] 2.2 Emit contiguous text blocks without crossing reasoning, tool, or
  provider-state boundaries.
- [x] 2.3 Preserve each thinking block as a block-local `ResponseItem::Reasoning`
  with its own signature.
- [x] 2.4 Add stream tests for interleaved text, thinking, tool_use, and
  provider-state blocks.

## 3. Redacted Thinking and Provider State

- [x] 3.1 Parse `redacted_thinking` as opaque Claude provider state instead of an
  unsupported visible text block.
- [x] 3.2 Re-emit preserved `redacted_thinking` blocks unchanged in the next
  Claude request.
- [x] 3.3 Decide whether `ResponseItem::Compaction` remains the provider-state
  carrier or whether a narrow `ProviderState` item is needed.
- [x] 3.4 Add round-trip tests for `thinking`, `signature_delta`,
  `redacted_thinking`, and provider-state ordering around tool calls.

## 4. Tool-Result Validation

- [x] 4.1 Replace unsafe same-role coalescing around `tool_result` with a Claude
  history builder or equivalent validation pass.
- [x] 4.2 Enforce that `tool_result` blocks immediately follow matching
  `tool_use` blocks and appear before ordinary user content.
- [x] 4.3 Preserve valid multiple parallel tool results in deterministic order.
- [x] 4.4 Return clear local errors for orphan, reordered, or delayed tool
  results.
- [x] 4.5 Add request-shape and mocked tool-loop tests for valid and invalid
  histories.

## 5. Stop Reasons and Continuation

- [x] 5.1 Add support for `model_context_window_exceeded`.
- [x] 5.2 Preserve unknown Claude stop reasons without stream parse failure.
- [x] 5.3 Keep continuation behavior limited to known `tool_use` and
  `pause_turn` semantics.
- [x] 5.4 Add tests for known, new, and unknown stop reasons.

## 6. Auth and Error Mapping

- [x] 6.1 Fix Claude 401 handling so unauthorized recovery sees Anthropic 401
  responses before generic API-error mapping hides them.
- [x] 6.2 Add tests for Claude 401 error-envelope recovery and recovery-exhausted
  behavior.
- [x] 6.3 Review Claude context-window and invalid-request error mapping for
  actionable local errors.

## 7. Token Usage Accounting

- [x] 7.1 Remap Claude cache usage so `cached_input_tokens` reflects cache-read
  input only and stays within shared `TokenUsage` invariants.
- [x] 7.2 Preserve or log `cache_creation_input_tokens` without treating it as
  already-cached input.
- [x] 7.3 Add usage tests for cache read, cache creation, combined cache fields,
  and clamped provider-compatible values.
- [x] 7.4 Verify post-turn Claude count-tokens context accounting still works.

## 8. Documentation and Feature Limits

- [x] 8.1 Update `docs/config.md` to state that Claude strict structured output
  is prompt-guided unless a future enforcement strategy is added.
- [x] 8.2 Document that OpenAI Responses server-side tools are not
  protocol-equivalent on Claude; client-side Claude tools remain supported.
- [x] 8.3 Add warnings, trace annotations, or tests for Claude strict-output
  downgrade behavior if implementation changes are needed.

## 9. Verification

- [x] 9.1 Run `cd codex-rs && just fmt`.
- [x] 9.2 Run `cd codex-rs && cargo test -p codex-api claude`.
- [x] 9.3 Run `cd codex-rs && cargo test -p codex-core claude`.
- [x] 9.4 Run `cd codex-rs && cargo test -p codex-tools claude` if touched.
- [x] 9.5 Run scoped `just fix -p <crate>` for changed Rust crates after tests
  pass.
