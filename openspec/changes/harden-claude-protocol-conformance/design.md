## Context

The Claude adapter currently owns the correct high-level boundary:

- `codex-api` owns the Claude endpoint and SSE accumulator.
- `codex-core` builds Claude Messages requests and dispatches by
  `WireApi::Claude`.
- `codex-tools` serializes Codex tools into Claude-compatible tool declarations.
- The shared tool loop still consumes and produces `ResponseItem` values.

This change keeps that boundary. The goal is not to introduce a full Claude
transcript model in shared protocol types; it is to preserve the minimum
provider-native state required to resend valid Claude Messages history.

External references:

- Anthropic tool-use docs require assistant `tool_use` blocks followed by user
  `tool_result` blocks, with tool results at the start of user content.
- Anthropic streaming docs define `input_json_delta`, `thinking_delta`, and
  `signature_delta` as per-content-block deltas.
- Anthropic extended-thinking docs require thinking signatures and
  `redacted_thinking` blocks to be preserved when resending assistant content.
- Cline's Anthropic provider handles thinking, signatures, redacted thinking,
  and message sanitization as provider-format concerns.
- LiteLLM and openai-agents issue reports show real failures when providers
  replay malformed `tool_use` / `tool_result` histories.

## Goals / Non-Goals

**Goals:**

- Preserve Claude assistant content-block order across streaming finalization and
  request replay.
- Round-trip `redacted_thinking` and provider-state blocks without exposing them
  as ordinary assistant text.
- Validate Claude tool-result adjacency before network send.
- Handle known and unknown Claude stop reasons without deserialization failures.
- Keep Claude 401 auth recovery aligned with the Responses path.
- Preserve internal token-accounting invariants for Claude cache usage.
- Make non-equivalent Claude features explicit in docs and tests.

**Non-Goals:**

- Do not route Claude through OpenAI Responses request or stream code.
- Do not add a dependency on Cline, LiteLLM, Anthropic SDKs, or another agent
  implementation.
- Do not add public Claude prompt-cache, beta-header, or structured-output
  config in this proposal.
- Do not implement a Claude Code CLI harness.
- Do not redesign the shared `ResponseItem` model unless ordered Claude replay
  cannot be achieved with a narrow adapter-owned representation.

## Technical Plan

### 1. Emit final Claude stream items in content-block order

Current behavior finalizes Claude streams by output category:

1. all reasoning,
2. all assistant text,
3. all tool calls,
4. all provider-state blocks.

That is safe for simple turns but unsafe for interleaved Claude assistant
content. Replace category finalization with an ordered finalization pass:

- keep a `BTreeMap<usize, ClaudeFinalBlock>` keyed by Claude content block
  index;
- build one final block when each `content_block_stop` arrives, or during
  `message_stop` for any still-open block;
- emit `ResponseEvent::OutputItemDone` in ascending block-index order.

`ClaudeFinalBlock` should distinguish at least:

- `Text { text }`
- `Thinking { text, signature }`
- `ToolUse { id, name, input }`
- `ProviderState { value }`

Contiguous text blocks MAY be coalesced into one assistant `Message` only when
no non-text Claude block sits between them. Thinking blocks SHOULD remain
block-local so each signature remains attached to the exact thinking block that
Anthropic produced.

Streaming deltas remain live:

- text deltas still emit `OutputTextDelta`;
- thinking deltas still emit `ReasoningContentDelta`;
- custom tool input deltas still emit `ToolCallInputDelta`.

Only final item ordering changes.

### 2. Round-trip `redacted_thinking` as opaque provider state

Extend Claude stream content-block parsing so `redacted_thinking` is not treated
as an unsupported visible block. It should be preserved as opaque Claude replay
state:

- parse the raw block value and store it as `ProviderState`;
- emit a provider-state `ResponseItem` at its original block index;
- serialize it back through the Claude request builder as a raw Claude content
  block with the same `type` and payload.

The existing `ResponseItem::Compaction { encrypted_content }` can be reused if
it continues to mean "provider-owned opaque state to be returned to the same
wire API". If that naming proves too misleading, add a narrow provider-neutral
item such as `ResponseItem::ProviderState` with documentation and schema tests.

Do not emit `redacted_thinking` as assistant text. Do not drop it silently.

### 3. Preserve thinking signatures per block

Change the Claude stream parser so each thinking block produces one
`ResponseItem::Reasoning` with:

- only that block's visible thinking text;
- only that block's accumulated signature;
- no concatenation with later thinking blocks.

The Claude request builder should replay each reasoning item as one Claude
`thinking` block in the order it appears in history. This keeps simple existing
single-thinking turns unchanged while making interleaved thinking/tool histories
safe.

If future Anthropic payloads require additional hidden fields beyond
`signature`, store a versioned JSON envelope in the provider-state path rather
than overloading the visible reasoning text.

### 4. Validate and sanitize Claude tool-result history

Replace raw same-role coalescing around tool blocks with a small Claude history
builder that tracks block classes and pending tool ids:

- `assistant` messages may contain text, thinking/provider-state, and
  `tool_use` blocks in provider order;
- a user message containing `tool_result` blocks MUST immediately follow the
  assistant message containing the matching `tool_use` ids;
- all `tool_result` blocks in that user message MUST appear before ordinary
  text or image blocks;
- ordinary user text MUST NOT be merged before a pending tool-result group;
- multiple parallel tool results must preserve the corresponding tool-use
  order where possible.

The builder should fail locally with a clear error when it sees:

- `tool_result` without a preceding unmatched `tool_use`;
- `tool_result.tool_use_id` that does not match any pending id;
- ordinary user content before required tool results;
- a new unrelated assistant/user boundary that would orphan pending tool calls.

Tests should include normal multiple-tool results, user text adjacent to tool
results, orphan tool results, reordered tool results, and pause-turn/provider
state between assistant blocks.

### 5. Make stop reasons forward compatible

Change `ClaudeStopReason` from a closed serde enum into either:

- an enum with `Unknown(String)` custom deserialization, or
- a raw `String` plus helper methods for known values.

Known behavior:

- `tool_use` and `pause_turn` set `end_turn = Some(false)`;
- `end_turn`, `max_tokens`, `stop_sequence`, `refusal`, and
  `model_context_window_exceeded` set `end_turn = Some(true)`;
- unknown values preserve `provider_stop_reason` and set `end_turn = None` so
  existing fallback logic can decide safely.

The parser MUST NOT fail just because Anthropic adds a stop reason.

### 6. Restore Claude 401 auth recovery

The shared core path currently retries unauthorized requests when it sees
`ApiError::Transport(TransportError::Http { status: 401, ... })`. The Claude
endpoint maps Anthropic JSON error envelopes before core handles the error, so
401 can become `ApiError::Api { status: 401, ... }`.

Use one of these approaches:

1. Prefer preserving `TransportError::Http` for 401 in
   `map_claude_api_error`, so the existing recovery path is reused unchanged.
2. If preserving transport shape is not viable, add a small shared helper in
   core that recognizes both transport-shaped and API-shaped 401 errors and
   invokes unauthorized recovery.

Add mocked tests for Claude 401 with an Anthropic error body and prove the
recovery path is exercised once before surfacing an auth failure.

### 7. Correct Claude cache usage mapping

Keep the shared `TokenUsage` invariant: `cached_input_tokens` represents the
cached subset of `input_tokens` used by display and blended-total math.

For Claude usage:

- map ordinary `input_tokens`, `output_tokens`, and total as before;
- map `cache_read_input_tokens` to `cached_input_tokens`, clamped to
  `input_tokens` if necessary;
- do not add `cache_creation_input_tokens` to `cached_input_tokens`;
- preserve cache creation visibility through telemetry or debug logs if needed,
  but do not overload the existing shared field.

Add unit tests for:

- cache read only;
- cache creation only;
- read plus creation;
- malformed/provider-compatible usage where cache read exceeds input tokens.

### 8. Make feature non-equivalence explicit

Claude structured output and server-side tools should be documented as
non-equivalent to OpenAI Responses:

- `output_schema_strict = true` on Claude is prompt-guided under the current
  adapter, not server-side strict validation;
- OpenAI Responses web/image-generation server tools are not automatically
  available through Claude Messages;
- client-side function/freeform/local shell/tool search remain available through
  Claude `tool_use`.

The implementation may add warnings or trace annotations when strict structured
output is requested for Claude. A future proposal can evaluate a synthetic
forced-tool strategy for strict final JSON if product requirements need it.

## Risks / Trade-offs

- Ordered finalization may change the sequence of `OutputItemDone` events for
  complex Claude responses. The test suite should lock intended behavior and
  downstream consumers should be audited for assumptions.
- Reusing `ResponseItem::Compaction` for `redacted_thinking` is expedient but
  semantically broad. If more Claude provider-state block types appear, a
  renamed provider-state item may be worth the protocol churn.
- Strict tool-result validation can reject historical transcripts that were
  previously sent to Anthropic and failed remotely. That is desirable only if
  the local error is clear and actionable.
- Unknown stop reasons must not trigger accidental continuation loops. Preserve
  the raw stop reason and avoid follow-up unless the known semantics require it.
- Cache-token remapping may change displayed blended totals for prompt-caching
  users; the new behavior should match the shared token model rather than
  Anthropic's raw billing categories exactly.

## Verification

- `openspec validate harden-claude-protocol-conformance --strict`
- `cd codex-rs && just fmt`
- `cd codex-rs && cargo test -p codex-api claude`
- `cd codex-rs && cargo test -p codex-core claude`
- `cd codex-rs && cargo test -p codex-tools claude` if tool metadata helpers
  change
- `cd codex-rs && just fix -p codex-api`
- `cd codex-rs && just fix -p codex-core`

If public protocol/schema types change, run the existing schema generation task
for the touched crate and include generated artifacts. If `ConfigToml` changes,
also run `cd codex-rs && just write-config-schema`.
