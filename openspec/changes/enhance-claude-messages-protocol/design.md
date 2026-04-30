## Context

The existing Claude adapter already owns the important protocol seams:

- `codex-tools` serializes Codex tool declarations into Claude-compatible
  `tools` entries and records reverse mapping metadata.
- `codex-api` owns the Claude HTTP endpoint and SSE accumulator.
- `codex-core` builds Claude Messages requests from `Prompt` and dispatches by
  `WireApi::Claude`.
- The shared Codex tool loop remains `ResponseItem` driven.

This change extends that same boundary. New Claude behavior should be added to
the Claude adapter and shared provider abstractions only where necessary. It
must not make the OpenAI Responses request builder or SSE parser understand
Claude-only fields.

## Goals / Non-Goals

**Goals:**

- Add Anthropic prompt caching support for stable Claude request prefixes.
- Add a Claude Messages token-counting endpoint client.
- Preserve protocol-relevant Claude content blocks that Codex cannot currently
  represent as ordinary text/image/tool/reasoning items.
- Handle `pause_turn` and compaction-like continuation state without breaking the
  existing client-side tool loop.
- Prove each boundary with typed unit tests and mocked endpoint tests.

**Non-Goals:**

- Do not implement a Claude Code CLI harness in this change.
- Do not copy Warp's Claude Code transcript or plugin-manager implementation
  into Codex's Claude wire adapter.
- Do not route Claude Messages through OpenAI Responses request or stream code.
- Do not call `/v1/messages/count_tokens` on every normal turn unless a caller
  explicitly needs a preflight estimate.
- Do not introduce a broad provider-specific transcript model if a small
  provider-neutral continuation item or adapter side table is sufficient.

## Reference Comparison: Warp

| Area | Warp behavior | Codex adaptation |
| --- | --- | --- |
| Claude Code launch | Runs the `claude` CLI with session/resume flags, prompt files, and local config preparation. | Out of scope. Codex's Claude Messages adapter talks directly to Anthropic HTTP. A future Claude Code harness should be a separate proposal. |
| Transcript rehydration | Reads/writes Claude Code JSONL, subagent transcripts, TODO files, and `sessions-index.json`. | Out of scope for raw `/v1/messages`. The useful lesson is to preserve provider state when resuming, which applies here to compaction/pause continuation blocks. |
| Plugin management | Installs/updates Warp's Claude Code plugin through the `claude plugin` CLI. | Out of scope. |
| Claude API guidance | Documents prompt caching, token counting, supporting endpoints, compaction, server-side tools, and Files/Batches. | Use this as a feature checklist, but implement only the protocol surfaces that fit Codex's current Claude adapter. |

## Decisions

### 1. Prompt caching is adapter-owned and conservative by default

The Claude request model should add typed cache metadata:

- `ClaudeCacheControl { kind: Ephemeral, ttl: Option<ClaudeCacheTtl> }`
- cache metadata on Claude tool definitions where Anthropic supports it;
- top-level `system` represented as either a string or typed text blocks so the
  last stable system segment can carry `cache_control`;
- message content blocks that may carry `cache_control`.

The initial policy should be conservative:

- keep the default disabled unless a caller explicitly enables the internal
  Claude request policy;
- when enabled, place the first breakpoint at the stable tools/system boundary;
- optionally place a conversation breakpoint on the latest prior message block
  when history is long enough to benefit;
- avoid marking per-turn volatile content such as user input, timestamps,
  request IDs, or dynamically generated tool lists.

If public config is added in a future change, prefer a self-documenting enum
over a boolean. The internal policy implemented here uses:

- `off`
- `system`
- `conversation`

TTL should also be explicit, for example `ephemeral_5m` or `ephemeral_1h`.

### 2. Token counting is a preflight capability, not a normal turn dependency

The Claude endpoint adapter should add a method for
`POST /v1/messages/count_tokens` that accepts the same prompt/history/tool
surface as a create request but omits fields that are not valid for counting.

This change stops at the endpoint-client boundary. A later proposal can decide
whether to thread the count result through a provider-neutral estimate API for:

- context-window admission checks for Claude providers;
- compaction decision points;
- manual diagnostics or tests.

Normal streaming turns should continue to use completion usage for accounting.

### 3. Preserve protocol-relevant content blocks without turning Codex into a Claude transcript model

The Claude stream parser currently handles text, tool use, thinking/signature,
and errors. New content block behavior should follow this order:

1. Add typed variants for Claude block types that Codex can project into an
   existing `ResponseItem` or `ContentItem`.
2. For provider state that must be returned in later Claude requests, preserve
   it in the smallest provider-neutral history shape possible.
3. For unsupported visual/user-facing blocks, emit an explicit placeholder or
   structured diagnostic rather than silently dropping the block.

This design intentionally avoids mirroring the entire Claude schema in the
shared protocol. The adapter owns Claude wire details; shared history only grows
when protocol correctness requires it.

### 4. Treat `pause_turn` separately from tool use

Claude's `tool_use` stop reason means Codex should execute client-side tools and
send `tool_result` blocks. `pause_turn` means Claude paused a long-running turn
and expects continuation with the assistant content it just emitted.

The stream accumulator should surface the specific stop reason internally. The
turn loop should:

- continue automatically when the provider signaled `pause_turn`;
- include the assistant content from the paused response in the next Claude
  request;
- cap automatic pause continuations to avoid infinite loops;
- continue using existing tool execution behavior for `tool_use`.

### 5. Compaction blocks must round-trip when enabled

If Claude emits compaction or other provider-state blocks, Codex must preserve
them in history so the next request can include them. This change does not add a
public beta-header configuration; any future Anthropic beta header needed to
request new Claude-only block types must be configured and applied only by
Claude endpoint adapters.

The current `ResponseItem::Compaction` may be reusable if the payload captures
the necessary provider state. If it is not sufficient, add a narrow
provider-neutral continuation item with clear documentation and schema updates.

### 6. Unknown block behavior must be visible in tests

Tests should prove that unsupported Claude blocks are not silently lost. The
desired behavior may be:

- typed conversion for supported block kinds;
- opaque round-trip for protocol-state blocks;
- explicit text placeholder for unsupported user-visible blocks;
- stream error only when the unknown shape makes the request impossible to
  continue safely.

## Risks / Trade-offs

- Prompt caching is easy to make technically correct but economically useless if
  breakpoints are placed after volatile content. Tests should compare complete
  request JSON for stable and volatile cases.
- A token-counting endpoint can improve estimates but introduces latency and
  another failure mode if future callers put it on the ordinary turn path.
- Provider-state preservation may pressure the shared protocol model. Keep new
  shared types narrow, documented, and tied to continuation requirements.
- Compaction and server-tool features may have changing beta header names. Keep
  future beta handling scoped to provider/endpoint boundaries.
- Continuation loops can be hard to reason about. Cap retries and test the cap.

## Verification

- `cd codex-rs && just fmt`
- `cd codex-rs && cargo test -p codex-api claude`
- `cd codex-rs && cargo test -p codex-core claude`
- `cd codex-rs && just fix -p codex-api`
- `cd codex-rs && just fix -p codex-core`

If config schema changes, also run:

- `cd codex-rs && just write-config-schema`

If protocol TypeScript/schema output changes, run the relevant existing schema
generation task for the touched crate.

If Rust dependencies change, also run from the repository root:

- `just bazel-lock-update`
- `just bazel-lock-check`
