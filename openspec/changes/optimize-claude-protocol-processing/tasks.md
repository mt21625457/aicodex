## 1. OpenSpec and Scope

- [x] 1.1 Validate this change against existing Claude OpenSpec proposals and
  keep it scoped to protocol processing optimization.
- [x] 1.2 Confirm no OpenAI Responses request or SSE parser changes are needed
  except provider-neutral error or telemetry helpers.
- [x] 1.3 Run `openspec validate optimize-claude-protocol-processing --strict`.

## 2. Usage Merge Correctness

- [x] 2.1 Update Claude stream usage merge so zero-valued input/cache fields do
  not overwrite earlier non-zero values.
- [x] 2.2 Preserve output token, server-tool usage, and iteration behavior with
  explicit tests.
- [x] 2.3 Add `codex-api` tests for `message_start` non-zero usage followed by
  zero-valued `message_delta`.
- [x] 2.4 Run `cd codex-rs && cargo test -p codex-api claude`.

## 3. Request Normalization Pipeline

- [x] 3.1 Add a deterministic Claude message normalization pass after
  `Prompt` conversion and before cache marking/validation.
- [x] 3.2 Normalize whitespace-only assistant messages and empty text blocks.
- [ ] 3.3 Normalize orphan/trailing thinking-only assistant content after
  validating the interaction with existing Claude signature replay tests.
- [x] 3.4 Sanitize error `tool_result` content so nested content is text-only.
- [x] 3.5 Preserve valid ordinary user content after required `tool_result`
  blocks while keeping tool results first.
- [x] 3.6 Add `codex-core` request-shape tests for recovered malformed
  histories.

## 4. Media Limits and Media Recovery

- [x] 4.1 Add Claude media counting for top-level media and nested media inside
  `tool_result.content`.
- [x] 4.2 Prune oldest excess media before sending while preserving recent media
  and all tool-result ids.
- [x] 4.3 Add placeholders when media pruning would empty a tool-result content
  block.
- [x] 4.4 Classify provider media errors for future targeted retry cleanup.
- [x] 4.5 Add `codex-core` tests for top-level and nested media pruning.

## 5. Streaming Resilience

- [x] 5.1 Classify Claude stream failures into idle, no-event, partial-close,
  provider-error, parse-error, and transport-error categories.
- [x] 5.2 Track whether a side-effecting tool call or tool input has already
  been emitted during the failed stream.
- [x] 5.3 Gate retries and any future non-streaming fallback so fallback cannot
  duplicate tool execution.
- [x] 5.4 Add mocked stream tests for empty stream, partial stream without
  content, partial stream after tool input, and provider error events.

## 6. History Replay Requirements

- [x] 6.1 Thread `ClaudeHistoryRequirements` from the Claude tool plan into
  request normalization and history retention decisions.
- [x] 6.2 Preserve server tool results, remote MCP results, and structured
  citation state when the active plan requires them.
- [x] 6.3 Drop or quarantine stale provider-state blocks only when the active
  plan proves they are no longer replayable.
- [ ] 6.4 Add tests for native server tools and remote MCP histories across
  follow-up turns and compaction.

## 7. Prompt Cache Extensions

- [x] 7.1 Keep existing `cache_control` request shapes unchanged and covered by
  tests.
- [x] 7.2 Add a provider capability design for cache references and cache edits
  before emitting new fields.
- [x] 7.3 Implement `cache_reference` on cached-prefix tool results only behind
  an explicit provider capability.
- [x] 7.4 Implement `cache_edits` insertion, dedupe, and pinning only after
  request-shape tests prove valid placement.
- [x] 7.5 Add tests proving unsupported Claude-compatible providers do not
  receive cache-edit fields.

## 8. Tool Input Normalization

- [x] 8.1 Normalize empty streamed JSON tool input to `{}` where appropriate.
- [x] 8.2 Preserve custom/freeform raw string input through the existing wrapper
  contract.
- [x] 8.3 Improve invalid JSON diagnostics with block index, tool name, and
  input length without logging raw input.
- [x] 8.4 Add `codex-api` tests for empty input, object input, raw custom input,
  and invalid input diagnostics.

## 9. Observability

- [ ] 9.1 Surface Claude upstream request id and relevant rate-limit/quota
  headers when available.
- [ ] 9.2 Add trace/debug counters for normalization repairs, media pruning,
  stream failure class, fallback gating, and cache usage details.
- [ ] 9.3 Ensure observability does not include raw prompts, tool inputs, media
  payloads, credentials, or provider-state payloads.
- [ ] 9.4 Add tests or telemetry fakes for header extraction and failure-class
  reporting where practical.

## 10. Verification

- [x] 10.1 Run `cd codex-rs && just fmt` after Rust edits.
- [x] 10.2 Run `cd codex-rs && cargo test -p codex-api claude` for stream and
  endpoint changes.
- [x] 10.3 Run `cd codex-rs && cargo test -p codex-core claude` for request,
  history, media, and turn orchestration changes.
- [x] 10.4 Run `cd codex-rs && cargo test -p codex-tools claude` if tool-plan
  metadata changes.
- [x] 10.5 Run scoped `just fix -p <crate>` for each touched Rust crate after
  tests pass.
- [ ] 10.6 Ask before running the complete Rust workspace test suite if common,
  core, or protocol changes require broader validation.
