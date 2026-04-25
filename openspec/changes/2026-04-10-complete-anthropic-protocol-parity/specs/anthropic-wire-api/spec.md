# Spec Delta: Anthropic Wire API

## MODIFIED Requirements

### Requirement: Anthropic stream output must map into existing internal response shapes

The Anthropic adapter MUST map stream output into the existing internal response and event model used by the rest of the system without losing protocol-required state needed for later Anthropic turns.

#### Scenario: Anthropic thinking is streamed before a tool call

- **GIVEN** an Anthropic streaming response containing thinking blocks and a later tool use
- **WHEN** the adapter processes the stream
- **THEN** it MUST preserve the raw thinking blocks needed to replay the Anthropic assistant turn faithfully
- **AND** it MUST still emit existing internal reasoning events/items compatible with current Codex turn handling

#### Scenario: Anthropic tool result contains structured multimodal content

- **GIVEN** a Codex tool result containing structured text and image items
- **WHEN** the Anthropic adapter reports that tool result back to Anthropic
- **THEN** it MUST encode the content as Anthropic-compatible tool result blocks
- **AND** it MUST NOT silently degrade the content to plain text when a faithful Anthropic block representation exists

### Requirement: Unsupported Anthropic features must be explicit

Features not implemented in the current Anthropic phase MUST be either filtered before Anthropic request construction or rejected explicitly at runtime.

#### Scenario: A tool is unsupported for Anthropic provider execution

- **GIVEN** a turn targeting an Anthropic provider
- **WHEN** the available tool set is prepared
- **THEN** tools that Anthropic cannot faithfully execute MUST NOT be exposed to the model for that turn

#### Scenario: Unsupported Anthropic history item reaches request construction

- **GIVEN** Anthropic request history contains an item that the adapter cannot faithfully replay
- **WHEN** the adapter attempts to build the Anthropic request
- **THEN** it MUST fail explicitly rather than silently dropping that item

## ADDED Requirements

### Requirement: Anthropic image input must be supported for user turns

The Anthropic adapter MUST support image content blocks for user messages when the incoming Codex turn contains image inputs.

#### Scenario: User message includes image and text

- **GIVEN** a user turn containing an image input and surrounding text
- **WHEN** the Anthropic request is constructed
- **THEN** the adapter MUST encode the user message as Anthropic-compatible multimodal content blocks
- **AND** it MUST preserve the relative order of image and text content

### Requirement: Anthropic tool-use input must only execute after block completion

The Anthropic adapter MUST NOT finalize or execute a tool call from partial `input_json_delta` fragments before the corresponding content block completes.

#### Scenario: Tool input JSON arrives incrementally

- **GIVEN** an Anthropic stream with `input_json_delta` events
- **WHEN** the tool-use block has not yet emitted `content_block_stop`
- **THEN** the adapter MUST continue buffering input
- **AND** it MUST NOT emit an executable internal tool-call item yet

#### Scenario: Tool use is truncated before block completion

- **GIVEN** an Anthropic stream ends with `stop_reason = "max_tokens"` before a tool-use block completes
- **WHEN** the adapter finishes processing the stream
- **THEN** it MUST report an incomplete-turn error
- **AND** it MUST NOT execute a partial tool call

### Requirement: Anthropic stop reasons must be handled explicitly

The Anthropic adapter MUST distinguish successful completion from stop reasons that require retry, continuation, or explicit failure.

#### Scenario: Anthropic response stops because of max token exhaustion

- **GIVEN** an Anthropic response with `stop_reason = "max_tokens"`
- **WHEN** the turn completes
- **THEN** the adapter MUST NOT report the response as a normal successful completion

#### Scenario: Anthropic response stops because of context window exhaustion

- **GIVEN** an Anthropic response with `stop_reason = "model_context_window_exceeded"`
- **WHEN** the adapter maps the result
- **THEN** it MUST surface a context-window-related failure compatible with existing Codex handling

### Requirement: Anthropic stream-side error events must map to meaningful retry semantics

Anthropic SSE `event:error` payloads MUST be classified into meaningful internal errors rather than being flattened into a generic unexpected status.

#### Scenario: Anthropic stream emits overloaded error event

- **GIVEN** an Anthropic SSE stream emits `event:error` with an `overloaded_error`
- **WHEN** the adapter processes that event
- **THEN** it MUST map the failure to an overload/capacity style internal error

### Requirement: Anthropic provider should honor service tier controls

Anthropic requests MUST map existing Codex per-turn service tier controls onto Anthropic request semantics when an equivalent exists.

#### Scenario: Turn requests a faster service tier

- **GIVEN** an Anthropic turn with a non-default Codex `service_tier`
- **WHEN** the Anthropic request is built
- **THEN** the adapter MUST encode the equivalent Anthropic `service_tier` field in the outgoing request

### Requirement: Anthropic token accounting must preserve cache usage

Anthropic token usage mapping MUST account for cache-related usage fields exposed by the provider.

#### Scenario: Anthropic response includes cache write and cache read token usage

- **GIVEN** an Anthropic response usage payload containing `cache_creation_input_tokens` and `cache_read_input_tokens`
- **WHEN** the adapter maps usage into internal token accounting
- **THEN** cached token totals MUST include both values instead of silently dropping one of them
