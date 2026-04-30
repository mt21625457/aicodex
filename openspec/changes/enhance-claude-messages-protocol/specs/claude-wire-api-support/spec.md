## ADDED Requirements

### Requirement: Claude prompt caching must be opt-in and preserve request semantics

Codex MUST support Anthropic `cache_control` metadata for Claude Messages
requests at Claude-supported stable prefix locations. Cache metadata MUST be
controlled by an explicit Claude/provider policy and MUST NOT be emitted for
volatile current-turn content by default. Enabling prompt caching MUST NOT change
the Codex tool loop, request roles, content ordering, or streamed response
semantics.

#### Scenario: Cache metadata is absent when disabled

- **WHEN** Claude prompt caching is disabled for a provider
- **THEN** the Claude Messages request contains no `cache_control` fields
- **AND** the serialized request remains compatible with the existing Claude
  Messages adapter behavior

#### Scenario: Stable tools and system prompt can be cached

- **WHEN** Claude prompt caching is enabled for stable prefix content
- **THEN** Codex marks eligible Claude tool definitions, system text blocks, or
  other stable prefix blocks with typed `cache_control` metadata
- **AND** the request keeps the same effective tools, system instructions, and
  message order as the uncached request
- **AND** per-turn user input, timestamps, request identifiers, and dynamically
  generated volatile content are not marked as cache breakpoints

#### Scenario: Conversation-prefix caching is conservative

- **WHEN** Claude prompt caching is configured to include conversation history
- **AND** the prior history is long enough and stable enough to benefit from a
  cache breakpoint
- **THEN** Codex MAY mark the latest eligible prior message content block with
  `cache_control`
- **AND** Codex MUST NOT mark the current user turn as cached prefix content

#### Scenario: Cache usage is visible in token accounting

- **WHEN** Claude returns cache creation or cache read usage fields
- **THEN** Codex maps those fields into token usage accounting without dropping
  ordinary input, output, or reasoning token counts
- **AND** mocked Claude SSE tests verify the mapped cache usage values

### Requirement: Claude providers must support native Messages token counting

Codex MUST support Anthropic's native
`POST /v1/messages/count_tokens` endpoint through the Claude endpoint client.
Token counting MUST reuse Claude Messages request serialization where possible
and MUST NOT become an unconditional extra network request for normal streaming
turns.

#### Scenario: Claude token counting sends the Anthropic count request

- **WHEN** a caller invokes the Claude endpoint client's token-count operation
- **THEN** Codex sends a request to `/v1/messages/count_tokens`
- **AND** the request includes the required Anthropic authentication and version
  headers
- **AND** the request body uses the same model, system, messages, tools, tool
  choice, thinking, and service settings that are valid for Claude token
  counting
- **AND** fields that are invalid for the count endpoint are omitted

#### Scenario: Token counting is only used at estimate call sites

- **WHEN** Codex performs an ordinary Claude streaming turn that does not need a
  preflight estimate
- **THEN** Codex does not call `/v1/messages/count_tokens`
- **AND** completion usage from the streaming response remains the source of
  final accounting

#### Scenario: Token counting failures degrade safely

- **WHEN** the Claude token-count endpoint returns an Anthropic error envelope,
  rate limit, overload, network failure, or malformed response
- **THEN** Codex surfaces an actionable count-token failure to the caller
- **AND** the OpenAI Responses path is not affected

### Requirement: Claude content blocks must be forward compatible

Codex MUST handle Claude Messages content blocks in a forward-compatible way.
Known Claude block types that Codex can project into existing response or
content items MUST be represented with typed variants. Claude provider-state
blocks that must be returned on a later request MUST be preserved explicitly.
Unsupported user-visible blocks MUST degrade to an explicit placeholder or
diagnostic rather than disappearing silently.

#### Scenario: Known additional Claude block types use typed handling

- **WHEN** Claude returns a supported non-text content block that Codex can map
  to an existing response, reasoning, tool, image, or provider-state item
- **THEN** the Claude stream accumulator records that block using a typed
  representation
- **AND** final Codex response items preserve the information required for user
  display, tool execution, or later Claude requests

#### Scenario: Unknown user-visible blocks are explicit

- **WHEN** Claude returns an unsupported user-visible content block
- **THEN** Codex emits an explicit placeholder or structured diagnostic for the
  unsupported block
- **AND** Codex does not silently omit the block from the assistant response

#### Scenario: Provider-state blocks can round-trip

- **WHEN** Claude returns a provider-state content block that must be included
  in a future Claude request
- **THEN** Codex preserves the block in the smallest provider-neutral history
  shape needed for protocol correctness
- **AND** the next Claude request re-emits the preserved block without routing it
  through OpenAI Responses request or stream parsing

### Requirement: Claude pause-turn continuations must preserve assistant state

Codex MUST distinguish Claude `pause_turn` stop reasons from `tool_use` stop
reasons. A paused Claude turn MUST continue only by sending the assistant
content emitted by the paused response back in the next Claude Messages request.
Automatic pause continuation MUST be bounded so repeated provider pauses cannot
loop forever.

#### Scenario: Tool use still drives the Codex tool loop

- **WHEN** Claude stops with `stop_reason = "tool_use"`
- **THEN** Codex executes the requested client-side tools through the existing
  tool loop
- **AND** the next Claude request contains matching user `tool_result` blocks
- **AND** pause-turn continuation logic is not used for that stop reason

#### Scenario: Pause turn continues with assistant content

- **WHEN** Claude stops with `stop_reason = "pause_turn"`
- **AND** the streamed response contains assistant content required for
  continuation
- **THEN** Codex includes that assistant content in the next Claude Messages
  request
- **AND** Codex continues the turn without requiring client-side tool execution
- **AND** the eventual final response is emitted as the same logical Codex turn

#### Scenario: Pause continuation is capped

- **WHEN** Claude repeatedly returns `pause_turn` without reaching completion or
  making progress
- **THEN** Codex stops automatic continuation after a bounded number of attempts
- **AND** Codex surfaces an actionable error that includes the provider stop
  reason

### Requirement: Claude compaction and provider state must round-trip when enabled

Codex MUST preserve Claude compaction or other provider-state content returned
by Claude when that content is required for later Claude Messages requests. Any
future Anthropic beta or feature headers required to request new Claude-only
blocks MUST be scoped to Claude endpoint adapters and MUST NOT leak into OpenAI
Responses requests.

#### Scenario: Compaction state survives across turns

- **WHEN** Claude emits a compaction or provider-state block that must be
  returned on a later request
- **THEN** Codex stores the state in history as an explicit provider-state item
  or another documented provider-neutral representation
- **AND** the next Claude request includes the state in the Claude-supported
  content block shape

### Requirement: Mock coverage must prove enhanced Claude protocol boundaries

Codex MUST add mocked endpoint and adapter tests for prompt caching, token
counting, content-block preservation, pause-turn continuation, and compaction
round-tripping. Tests MUST assert complete request shapes at protocol boundaries
where field placement, headers, stop reasons, or preserved content affect
correctness.

#### Scenario: Prompt caching request shapes are tested

- **WHEN** Claude prompt caching support is implemented
- **THEN** tests compare complete serialized Claude requests for cache disabled,
  stable-prefix cache, conversation-prefix cache, and volatile-content no-cache
  cases

#### Scenario: Token-count endpoint behavior is tested

- **WHEN** Claude token counting support is implemented
- **THEN** tests cover request path, headers, valid count request body, success
  response parsing, Anthropic error mapping, and proof that ordinary streaming
  requests do not invoke the count endpoint

#### Scenario: Continuation and provider-state behavior is tested end to end

- **WHEN** Claude continuation or provider-state support is implemented
- **THEN** mocked end-to-end tests prove `pause_turn` continuation eventually
  completes
- **AND** tests prove the continuation cap surfaces an error
- **AND** tests prove preserved provider-state blocks are emitted in the next
  Claude request
