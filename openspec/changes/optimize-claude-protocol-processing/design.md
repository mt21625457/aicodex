## Context

This proposal follows up on existing Claude wire support, conformance
hardening, native tool, and count-token work. It is based on a behavior-level
comparison with the local Claude reference implementation inspected for this
change, especially:

- `src/services/api/claude.ts` for stream usage merge, media pruning,
  prompt-cache breakpoints, and streaming fallback behavior;
- `src/utils/messages.ts` for request-time message normalization and
  tool-result pairing;
- `src/remote/sdkMessageAdapter.ts` for shape-based tool-result detection in
  remote/direct-connect histories;
- `src/QueryEngine.ts` for cumulative stream usage handling.

The goal is not to copy that implementation or its product-specific gates. The
goal is to port the protocol invariants that apply to Codex's Rust adapter and
to keep them inside Claude-owned adapter boundaries.

Relevant local ownership:

- `codex-rs/codex-api/src/sse/claude.rs` owns Claude stream parsing and
  stream-completion usage.
- `codex-rs/codex-api/src/endpoint/claude_messages.rs` owns Claude HTTP
  request/response mapping.
- `codex-rs/core/src/claude.rs` owns conversion from Codex `Prompt` history to
  Claude Messages request bodies.
- `codex-rs/tools/src/tool_spec.rs` owns Claude tool declarations, beta
  headers, native tool planning, and `ClaudeHistoryRequirements`.
- `codex-rs/core/src/session/turn.rs` owns provider-stream retry and
  turn-level fallback decisions.

## Goals / Non-Goals

**Goals:**

- Fix token usage undercounting caused by zero-valued stream deltas.
- Add request-time Claude history normalization for recovered or compacted
  sessions.
- Bound Claude media payloads before request send and prepare targeted media
  recovery.
- Classify Claude stream failures and gate fallback so tool execution is not
  duplicated.
- Use Claude tool-plan replay requirements during history retention and
  normalization.
- Define a safe path for cache references and cache edits without enabling
  them broadly.
- Make streamed tool input normalization and diagnostics clearer.
- Improve provider observability without logging sensitive payloads.

**Non-Goals:**

- Do not route Claude Messages through OpenAI Responses request or stream code.
- Do not add public user-facing provider settings unless a later phase needs
  them.
- Do not copy Claude Code internal feature gates, advisor product behavior,
  AFK/fast-mode policy, task-budget controls, or Bedrock-only body extensions.
- Do not implement cache editing until provider capabilities and request shapes
  are specified and tested.
- Do not silently drop provider-state blocks unless the active Claude tool plan
  proves they are stale and replaying them would be invalid.

## Technical Plan

### 1. Fix Claude usage merging

Change `ClaudeUsage::merge` so input-related fields are updated only when the
incoming field is present and greater than zero:

- `input_tokens`
- `cache_read_input_tokens`
- `cache_creation_input_tokens`

Keep output and metadata behavior separate:

- `output_tokens` MAY update to the latest present value because Anthropic
  output usage is cumulative across deltas;
- `server_tool_use` and `iterations` should preserve latest present values;
- future usage fields should be parsed without changing `TokenUsage`
  invariants until shared protocol support exists.

Add tests in `codex-api`:

- `message_start` with `input_tokens = 100`, `cache_read_input_tokens = 30`,
  followed by `message_delta` with both fields at `0`, completes with
  `input_tokens = 100` and `cached_input_tokens = 30`;
- `message_delta` with a later non-zero cache value updates the earlier value;
- absent usage still does not emit zero token usage.

### 2. Add a Claude request normalization pipeline

Introduce a narrowly scoped normalization function after raw Claude message
conversion and before prompt-cache marking and validation:

```text
Prompt history -> ClaudeMessage blocks -> normalize -> cache policy -> validate
```

The normalizer should be deterministic and testable. Suggested phases:

1. Remove invalid empty text blocks and whitespace-only assistant messages when
   they do not carry other content.
2. Strip trailing thinking-only blocks from the final assistant message when
   they cannot be validly continued.
3. Drop orphan thinking-only assistant messages that have no sibling assistant
   content from the same provider response.
4. Ensure non-final assistant messages have non-empty content, using a local
   placeholder only when dropping the message would break role alternation.
5. Sanitize `is_error` tool results so nested content is text-only.
6. Keep `tool_result` blocks before ordinary user blocks and preserve valid
   ordinary text/image content after the required results.
7. Filter stale provider-state blocks according to the active tool plan and
   `ClaudeHistoryRequirements`.

The existing tool-result repair should remain focused on client-side
`tool_use` / `tool_result` adjacency. The new normalizer should not absorb
that responsibility into unrelated cleanup code.

### 3. Enforce media limits and media recovery

Add a media counting helper for Claude request messages:

- count `ClaudeContentBlock::Image`;
- count image/document-like provider blocks if future typed document support is
  added;
- count nested media inside `ClaudeToolResultContent::Blocks`.

Add a conservative provider-level limit. If no explicit provider limit exists,
start with a named internal constant that matches the strictest Claude provider
the adapter supports, and document it in code.

Pruning policy:

- remove oldest media first;
- preserve the most recent media because it is usually most relevant;
- preserve all text and tool-result ids;
- if pruning empties a tool-result content block, replace it with a text
  placeholder that explains media was omitted.

Recovery policy:

- do not add automatic retry in the first media-limit patch;
- add structured error classification so a future retry can strip only media
  from the message that caused "too large", invalid image, or invalid document
  errors.

### 4. Classify stream failures and gate fallback

Add a Claude stream failure classification that can flow from `codex-api` to
turn orchestration:

- `IdleTimeout`
- `ClosedBeforeMessageStart`
- `ClosedAfterMessageStartBeforeStop`
- `ProviderError`
- `ParseError`
- `TransportError`

Track whether any side-effecting item has been emitted:

- client tool call started;
- custom/freeform tool input emitted;
- local shell/native edit/apply-patch call emitted;
- any tool output has already been submitted.

Fallback policy:

- safe to consider non-streaming fallback when no assistant item and no tool
  call has been emitted;
- safe to retry normal stream according to existing retry budget when no
  side-effecting tool has started;
- unsafe to fallback after partial tool streaming unless a future idempotency
  token proves the duplicate tool execution cannot occur;
- preserve user abort as a user abort, not a fallback trigger.

The first implementation may stop at classification and clearer retry
diagnostics. Non-streaming fallback should be a later phase guarded by mocked
tests.

### 5. Use `ClaudeHistoryRequirements` for retention

Thread the `history_requirements` from `ClaudeToolsJson` into the place where
Claude request history is normalized and where compaction decides what can be
dropped.

Rules:

- when `preserve_server_tool_results` is true, provider-state blocks for
  server-side tool use/result pairs must remain in provider order;
- when `preserve_mcp_tool_results` is true, remote MCP `mcp_tool_use` and
  `mcp_tool_result` blocks must not be locally converted or dropped;
- when `preserve_structured_citations` is true, citation provider state must
  remain available for replay and display;
- when the active tool plan no longer enables a provider-owned tool, stale
  provider-state blocks may be dropped only if tests prove the provider would
  reject them or they are no longer meaningful.

This should not create a broad shared provider-state API unless Claude cannot
remain adapter-owned.

### 6. Extend prompt cache policy safely

Keep current `cache_control` behavior stable. Add a design-only capability
layer before enabling cache edits:

- provider capability flag for cache editing;
- typed request representation for `cache_reference` on cached-prefix
  `tool_result` blocks;
- typed request representation for `cache_edits` deletion blocks;
- dedupe cache edit references before send;
- ensure exactly one active cache-control breakpoint unless a provider-specific
  design proves multiple markers are valid and beneficial;
- avoid mutating shared history blocks when adding cache-only request metadata.

Cache editing should not be enabled for DeepSeek-compatible Claude providers
or other compatible gateways until they explicitly advertise support.

Initial capability design:

- keep `cache_control` as the only enabled prompt-cache request metadata for
  now;
- model cache editing as a provider capability that defaults to disabled and
  is independent of the existing `ClaudePromptCacheMode`;
- represent future `cache_reference` metadata as an optional field on
  `tool_result` blocks, never as a mutation of stored conversation history;
- represent future `cache_edits` deletion requests as user-message content
  blocks inserted after required `tool_result` blocks and before ordinary user
  content;
- deduplicate `cache_edits` by cache reference before serialization;
- gate all `cache_reference` and `cache_edits` emission on the provider
  capability so Claude-compatible providers such as DeepSeek continue to
  receive only the current `cache_control` shapes.

### 7. Normalize Claude tool input

Add a helper in the Claude SSE accumulator for final tool input:

- streamed empty string -> `{}` for function/native JSON-object tools;
- streamed JSON object -> object;
- streamed JSON string for custom/freeform tools -> raw input wrapper used by
  existing custom tool mapping;
- invalid JSON -> stream error that includes content block index, Claude tool
  name if known, and input length.

Do not log raw input payload by default. Tests should verify the diagnostic
shape without including sensitive content.

### 8. Add Claude observability hooks

Observability should help diagnose protocol problems without exposing user
payload:

- upstream request id from Claude response headers;
- selected Claude beta headers and native tool policy decisions;
- normalization repair counters by kind;
- media pruning counts by location;
- stream failure class and whether fallback was considered or blocked;
- rate-limit/quota headers when available and provider-neutral enough to
  expose;
- cache read/create/delete usage fields in telemetry, without changing shared
  `TokenUsage` semantics.

Prefer trace/debug metadata over user-visible messages unless the user needs to
act.

## Implementation Order

1. Usage merge correctness.
2. Request normalization pipeline.
3. Media limit and media pruning.
4. Stream failure classification and safe fallback gates.
5. `ClaudeHistoryRequirements` retention plumbing.
6. Cache-reference/cache-edit design and guarded implementation.
7. Tool input normalization diagnostics.
8. Header/rate-limit/repair observability.

Each step should land with focused tests before the next step broadens the
surface area.

## Test Strategy

- `cd codex-rs && cargo test -p codex-api claude` for stream parsing, usage,
  failure classification, and tool-input diagnostics.
- `cd codex-rs && cargo test -p codex-core claude` for request normalization,
  media pruning, history retention, and mocked Claude tool-loop behavior.
- `cd codex-rs && cargo test -p codex-tools claude` if tool-plan metadata or
  `ClaudeHistoryRequirements` changes.
- `cd codex-rs && just fmt` after Rust edits.
- `cd codex-rs && just fix -p codex-api`, `just fix -p codex-core`, or
  `just fix -p codex-tools` for touched crates after tests pass.

Full workspace tests should be requested separately if shared protocol or core
turn orchestration changes make broad coverage necessary.
