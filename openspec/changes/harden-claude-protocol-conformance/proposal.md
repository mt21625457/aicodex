## Why

The current Claude Messages adapter is architecturally separated from OpenAI
Responses and already supports the main `/v1/messages` tool loop. A follow-up
protocol audit found several conformance gaps that matter for production Claude
turns, especially with extended thinking, provider continuation state, strict
tool-result ordering, and newer Anthropic stop reasons.

The most important risks are:

- Claude extended thinking requires assistant thinking blocks, signatures, and
  `redacted_thinking` blocks to be preserved and replayed without reordering.
  The current stream finalizer groups reasoning, text, tools, and provider state
  by category, which can change the original Claude content-block order.
- Claude requires `tool_result` blocks to immediately follow the matching
  assistant `tool_use` blocks, and user content containing tool results must not
  put ordinary text before those results. Current history coalescing can hide
  unsafe mixtures if unusual history is replayed.
- Anthropic has added stop reasons such as `model_context_window_exceeded`.
  Unknown stop reasons currently risk deserialization failure instead of being
  surfaced as provider-native stop metadata.
- Claude HTTP error mapping can convert `401` responses into generic API errors
  before the shared unauthorized-recovery path sees them.
- Claude cache usage fields are not semantically identical to OpenAI cached
  input tokens. Mapping cache creation tokens as cached input can make UI and
  accounting math misleading.
- Claude does not provide the same strict structured-output or OpenAI
  server-side tool semantics as Responses; Codex should make these limitations
  explicit rather than implying full feature parity.

This proposal hardens the Claude adapter without changing OpenAI Responses
behavior or moving Claude-specific logic into the shared Responses parser.

## What Changes

- Preserve Claude stream content blocks in provider order:
  - emit final `ResponseItem`s in Claude content-block index order;
  - keep each thinking block's signature with that block;
  - preserve `redacted_thinking` and provider-state blocks as opaque Claude
    replay content instead of converting them to visible placeholder text;
  - keep streaming text/reasoning deltas unchanged for UI responsiveness.
- Add a Claude history validation/sanitization pass before sending requests:
  - enforce `tool_result` adjacency and ordering;
  - avoid coalescing ordinary user text into unsafe tool-result messages;
  - return a clear local error for invalid replay history instead of sending an
    Anthropic-invalid request.
- Make Claude stop-reason handling forward compatible:
  - support `model_context_window_exceeded`;
  - preserve unknown stop reasons as raw provider strings without failing stream
    parsing.
- Fix Claude unauthorized recovery:
  - preserve or explicitly handle `401` status before Claude error-envelope
    mapping hides it from the shared auth recovery path.
- Correct Claude cache-token accounting semantics:
  - keep internal `TokenUsage.cached_input_tokens` consistent with the existing
    "cached subset of input" invariant;
  - avoid counting Claude cache creation tokens as already-cached input unless
    the shared token model is deliberately expanded.
- Document and test Claude feature limitations:
  - `output_schema_strict` is prompt-guided for Claude unless a future native or
    synthetic-tool strategy is implemented;
  - OpenAI Responses server-side web/image generation is not protocol-equivalent
    on Claude.

## Capabilities

### New Capabilities

- `claude-protocol-conformance-hardening`: Codex can preserve Claude-native
  continuation and thinking state with stricter request validation and
  forward-compatible stop-reason handling.

### Modified Capabilities

- `claude-wire-api-support`: Claude Messages routing gains ordered block replay,
  strict tool-result validation, redacted-thinking round-trip, unauthorized
  recovery parity, cache usage invariant fixes, and explicit feature-limit
  documentation.

## Impact

- Affected crates:
  - `codex-rs/codex-api`
  - `codex-rs/core`
  - `codex-rs/tools` only if tool-result validation needs helper metadata
- Affected docs:
  - `docs/config.md`
- Compatibility:
  - OpenAI Responses request construction and SSE parsing are unchanged.
  - Existing Claude happy-path text/tool turns keep the same runtime semantics.
  - Some malformed replay histories that Anthropic would reject remotely become
    local validation errors with clearer messages.
- Primary risks:
  - Emitting final items in provider order may expose assumptions in downstream
    display or history code that expected reasoning before tools.
  - Preserving opaque Claude blocks must stay scoped to Claude replay and not
    become a broad provider-transcript model.
  - Cache-token mapping changes may alter UI totals for prompt-caching sessions;
    tests must lock the intended invariant.
- Rollback:
  - Disable the stricter validator only if it rejects valid Claude histories.
  - Revert ordered replay to the previous category grouping while retaining
    stop-reason and auth fixes if needed.
