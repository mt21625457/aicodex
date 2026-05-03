## ADDED Requirements

### Requirement: Claude stream finalization must preserve provider block order

Codex MUST preserve Claude assistant content-block order when converting a
Claude Messages stream into final `ResponseItem`s for history replay. The
Claude stream parser MUST NOT reorder thinking, text, tool-use, or provider
state blocks by output category when that would change the order required by a
future Claude request.

#### Scenario: Interleaved thinking and tool use keeps original order

- **WHEN** Claude streams assistant content as thinking, tool_use, thinking, and
  tool_use blocks in that order
- **THEN** Codex emits final response items in the same provider block order
- **AND** the next Claude request replays those assistant blocks in the same
  order
- **AND** each thinking block keeps its own signature metadata

#### Scenario: Text coalescing does not cross non-text blocks

- **WHEN** Claude streams multiple text blocks separated by thinking, tool_use,
  or provider-state blocks
- **THEN** Codex MAY coalesce contiguous text blocks
- **BUT** Codex MUST NOT move text across non-text Claude block boundaries in
  replay history

### Requirement: Claude redacted thinking must round-trip opaquely

Codex MUST preserve Claude `redacted_thinking` blocks as provider-owned replay
state. Redacted thinking MUST NOT be emitted as ordinary visible assistant text
and MUST NOT be silently dropped.

#### Scenario: Redacted thinking survives a tool continuation

- **WHEN** Claude streams a `redacted_thinking` block before or between tool_use
  blocks
- **THEN** Codex stores the raw block payload in history as Claude provider
  state
- **AND** the next Claude request includes the same `redacted_thinking` block at
  the same relative assistant-content position

### Requirement: Claude tool-result history must be validated before send

Codex MUST validate Claude tool-result history before sending a Claude Messages
request. A user message containing `tool_result` blocks MUST immediately follow
the assistant message containing the matching `tool_use` blocks, and the
`tool_result` blocks MUST appear before ordinary user text or image content in
that message.

#### Scenario: Valid parallel tool results are preserved

- **WHEN** an assistant message contains multiple Claude `tool_use` blocks
- **AND** the following user message contains matching `tool_result` blocks
- **THEN** Codex sends the results in deterministic order without inserting
  ordinary text before them

#### Scenario: Orphan tool result fails locally

- **WHEN** prompt history contains a `tool_result` without a preceding unmatched
  Claude `tool_use`
- **THEN** Codex fails request construction with a clear local error
- **AND** Codex does not send an invalid request to Anthropic

#### Scenario: User text is not merged before tool results

- **WHEN** ordinary user text is adjacent to tool-result history
- **THEN** Codex MUST NOT coalesce that text ahead of required `tool_result`
  blocks
- **AND** any allowed text after tool results MUST preserve Claude's
  tool-result ordering requirements

### Requirement: Claude stop reason parsing must be forward compatible

Codex MUST preserve Claude provider stop reasons as raw provider metadata and
MUST NOT fail stream parsing solely because Anthropic adds a new stop reason.

#### Scenario: Model context-window stop reason is handled

- **WHEN** Claude returns `stop_reason = "model_context_window_exceeded"`
- **THEN** Codex completes the stream without a deserialization failure
- **AND** the completed event includes
  `provider_stop_reason = "model_context_window_exceeded"`
- **AND** Codex does not trigger tool or pause-turn continuation for that stop
  reason

#### Scenario: Unknown stop reason is preserved

- **WHEN** Claude returns a stop reason Codex does not yet know
- **THEN** Codex preserves the raw stop reason in the completed event
- **AND** Codex does not assume a follow-up tool loop or pause continuation
- **AND** stream parsing remains successful if the rest of the stream is valid

### Requirement: Claude unauthorized responses must reach auth recovery

Codex MUST ensure Claude `401 Unauthorized` responses can trigger the shared
unauthorized-recovery path. Claude-specific Anthropic error-envelope mapping
MUST NOT hide 401 status from recovery logic before recovery has a chance to
run.

#### Scenario: Anthropic 401 error envelope retries through auth recovery

- **WHEN** a Claude Messages request returns HTTP 401 with an Anthropic JSON
  error envelope
- **THEN** Codex invokes the configured unauthorized-recovery path once when
  recovery is available
- **AND** the retry uses refreshed auth context
- **AND** OpenAI Responses 401 behavior remains unchanged

### Requirement: Claude feature limitations must be explicit

Codex MUST document provider features that are not protocol-equivalent between
OpenAI Responses and Claude Messages. Claude support MUST NOT imply server-side
strict JSON-schema validation or OpenAI Responses server-tool parity unless a
specific Claude implementation strategy exists.

#### Scenario: Strict structured output is downgraded explicitly

- **WHEN** a Claude provider receives a prompt with `output_schema_strict = true`
- **THEN** Codex documents or reports that the current Claude adapter uses
  prompt-guided JSON output rather than OpenAI Responses server-side strict
  validation
- **AND** tests or documentation cover this difference

#### Scenario: Server-side OpenAI tools are not advertised as Claude-native

- **WHEN** a Claude provider is active
- **THEN** OpenAI Responses server-side tools such as hosted web search or image
  generation are documented as non-equivalent
- **AND** client-side function, freeform, local shell, and tool-search behavior
  remains available through Claude `tool_use` blocks

### Requirement: Claude cache usage must preserve TokenUsage invariants

Codex MUST map Claude cache usage fields into the shared `TokenUsage` model
without breaking the model's existing invariant that `cached_input_tokens` is a
cached subset of `input_tokens` used for display and blended-total calculations.

#### Scenario: Cache read tokens are counted as cached input

- **WHEN** Claude usage includes `cache_read_input_tokens`
- **THEN** Codex maps those tokens to `cached_input_tokens`
- **AND** `cached_input_tokens` does not exceed `input_tokens`

#### Scenario: Cache creation tokens are not treated as cached input

- **WHEN** Claude usage includes `cache_creation_input_tokens`
- **THEN** Codex preserves ordinary input and output token counts
- **AND** Codex does not add cache creation tokens to `cached_input_tokens`
- **AND** any cache creation visibility is handled through a separate telemetry,
  diagnostic, or future accounting field rather than overloading cached input

#### Scenario: Claude context usage refresh remains compatible

- **WHEN** Claude post-turn context usage is refreshed through
  `/v1/messages/count_tokens`
- **THEN** the refreshed context-window usage continues to use the native input
  token count or local fallback estimate
- **AND** stream usage cache-field mapping does not cause an empty or negative
  context usage update
