## Why

The Claude Messages adapter is now functionally separated from OpenAI
Responses and already handles the core Claude wire path. A comparison with the
local Claude reference implementation inspected for this change found a second
set of production-hardening gaps. These gaps are less about first support and
more about long-lived session recovery, protocol edge cases, usage correctness,
and safe fallback behavior.

The current adapter already has strong pieces:

- typed Claude request construction in `codex-core`;
- typed Claude endpoint and SSE parsing in `codex-api`;
- Claude tool planning and native tool metadata in `codex-tools`;
- tool-result repair for orphan, missing, duplicated, and reordered
  client-side tool history;
- provider-state replay for Claude-only blocks required by follow-up turns.

The remaining risks are concentrated in these areas:

- Anthropic streaming usage can report input/cache fields at `message_start`
  and later send explicit zeroes in `message_delta`. Merging those zeroes over
  the original values undercounts context and cost.
- Claude request history still needs a broader normalization pass for malformed
  recovered sessions: orphan/trailing thinking, whitespace-only assistant
  blocks, empty assistant messages, media inside error tool results, and stale
  provider-state blocks.
- Claude media limits and "too large" recovery are not yet enforced before
  send, so a bad resumed session can repeatedly fail with the same invalid
  image or document payload.
- The stream path detects idle and premature close errors, but fallback and
  retry behavior is not fine-grained enough to separate safe empty-stream
  fallback from unsafe mid-tool fallback.
- Claude native tool planning records history replay requirements, but those
  requirements are not yet a first-class input to history retention,
  compaction, and normalization decisions.
- Prompt caching exists at a basic cache-control level, but cache editing,
  cache references on cached-prefix tool results, and cache-break diagnostics
  need a deliberate design before implementation.
- Tool input normalization and observability can be improved without changing
  the shared tool loop.

This proposal stages those improvements in the recommended implementation
order so each step is independently testable and can land without widening the
Claude adapter into the OpenAI Responses path.

## What Changes

- Correct Claude streaming usage merge semantics:
  - preserve non-zero `input_tokens`, `cache_read_input_tokens`, and
    `cache_creation_input_tokens` from earlier stream events when later events
    report zero;
  - keep output tokens, server-tool usage, and iteration metadata
    forward-compatible;
  - add unit tests for `message_start` followed by zero-valued
    `message_delta`.
- Add a Claude message normalization pass before request validation:
  - strip or repair orphan/trailing thinking-only assistant content;
  - remove whitespace-only assistant text blocks or replace non-final empty
    assistant content with a local placeholder;
  - sanitize `is_error` tool results so their nested content is text-only;
  - keep `tool_result` blocks first in user content and preserve valid
    ordinary user content after required tool results;
  - filter provider-state blocks that are no longer replayable for the active
    Claude tool plan.
- Enforce Claude media limits and media recovery:
  - count top-level image/document-like blocks and nested media inside
    `tool_result.content`;
  - prune oldest excess media before sending while preserving recent context;
  - allow targeted removal of media after provider "too large", invalid image,
    or invalid document errors when a future retry policy records that signal.
- Improve streaming resilience:
  - classify idle, no-event, partial-before-stop, parse, and provider error
    failures;
  - retry or fallback only when doing so cannot duplicate already-started
    tool execution;
  - keep mid-tool failures on the existing retry/error path unless a future
    idempotency mechanism proves the retry is safe.
- Wire `ClaudeHistoryRequirements` into history retention:
  - use the tool plan's replay requirements to decide which provider-state,
    server-tool result, MCP result, and structured citation blocks must survive
    compaction and normalization;
  - test native server tool and remote MCP histories with and without the
    corresponding tools enabled.
- Extend prompt-cache policy deliberately:
  - keep current cache-control behavior stable;
  - design cache-reference and cache-edit support behind provider capability
    checks;
  - avoid adding cache-edit fields to providers that do not explicitly support
    them.
- Improve Claude tool input normalization:
  - treat empty streamed tool input as an empty object where the Claude tool
    schema expects JSON object input;
  - keep custom/freeform raw string input wrapped explicitly;
  - return diagnostics that include block index, tool name, and input length
    without logging sensitive payloads.
- Improve Claude protocol observability:
  - surface upstream request ids and relevant rate-limit or quota headers when
    available;
  - add trace fields for normalization repairs, media pruning, stream failure
    classes, and safe-fallback decisions.
- Document non-goals:
  - do not copy Claude Code internal feature gates or product-only betas;
  - do not move Claude protocol behavior into OpenAI Responses request or SSE
    parsing;
  - do not expose new public provider config until the protocol behavior is
    stable and tested.

## Capabilities

### New Capabilities

- `claude-protocol-processing-optimization`: Codex can normalize, account,
  retry, cache, and observe Claude Messages protocol processing in a way that
  is robust for long-lived sessions and compatible providers.

### Modified Capabilities

- `claude-wire-api-support`: Claude Messages request building and stream
  processing gain additional normalization, usage, media, fallback, history,
  cache, tool-input, and observability guarantees.
- `claude-context-usage-accounting`: Claude context and completion usage must
  preserve non-zero input/cache fields across stream events and continue to
  degrade safely when provider accounting is incomplete.

## Impact

- Affected crates:
  - `codex-rs/codex-api` for SSE usage merge, stream error classification,
    header observability, and safe fallback plumbing.
  - `codex-rs/core` for Claude request normalization, media pruning,
    history retention, retry/fallback orchestration, and integration tests.
  - `codex-rs/tools` for exposing Claude tool-plan replay requirements where
    the request normalizer and compaction path need them.
- Affected docs:
  - OpenSpec change records under `openspec/changes`.
  - Public docs only if new provider-visible cache or fallback config is later
    introduced.
- Compatibility:
  - OpenAI Responses behavior is unchanged.
  - Claude happy-path text/tool turns keep the same user-visible semantics.
  - Malformed replay histories that would fail remotely may be repaired or
    rejected locally with clearer diagnostics.
- Primary risks:
  - Over-aggressive normalization could drop valid Claude provider-state.
  - Unsafe fallback after partial tool streaming could duplicate tool
    execution if not gated tightly.
  - Cache-edit support could send Anthropic-only fields to compatible
    non-Anthropic providers unless capability checks are strict.
- Rollback:
  - Usage merge fixes can remain even if later phases roll back.
  - Normalization and media pruning should be feature-internal and can be
    bypassed if they reject valid histories.
  - Non-streaming fallback should stay disabled by default until safe-fallback
    tests cover each failure class.
