# chat-completions-wire-api-support Specification

## Purpose
TBD - created by archiving change unify-multi-backend-sampling-normalization. Update Purpose after archive.
## Requirements
### Requirement: Chat Completions providers MUST use the Chat Completions wire API

Codex MUST route providers configured with `WireApi::Chat` (config value `"chat"`) to
the OpenAI Chat Completions endpoint rather than the OpenAI Responses or Claude Messages
endpoints. With a default OpenAI-compatible base URL ending at `/v1`, the request path
MUST resolve to `chat/completions`. Chat requests MUST use the provider's configured
auth scheme for that provider (typically `Authorization: Bearer`) and MUST request an
event stream for streaming turns.

#### Scenario: Chat provider sends a streaming request

- **WHEN** a model provider is configured with `WireApi::Chat`
- **THEN** Codex sends a streaming request to `chat/completions`
- **AND** the request includes `Accept: text/event-stream` (or equivalent streaming accept header used by the Codex HTTP stack)
- **AND** the request does not use the OpenAI Responses request body schema
- **AND** the request does not use the Claude Messages request body schema

#### Scenario: Chat logic stays out of other parsers

- **WHEN** Chat Completions streaming is implemented
- **THEN** Chat chunk parsing lives in a Chat-specific L2 module
- **AND** the Responses SSE parser and Claude SSE parser are not edited to understand Chat chunk shapes

### Requirement: Chat history MUST serialize as valid Chat Completions messages

Codex MUST translate internal `Prompt` history into Chat Completions `messages`.
System / developer / base instructions MUST be represented using Chat-compatible roles
(`system` and/or documented developer mapping). User and assistant turns MUST use
`user` / `assistant` roles. Prior tool calls MUST serialize as assistant messages
containing `tool_calls`, and tool outputs MUST serialize as follow-up `tool` role
messages (or the documented Chat-compatible equivalent) that preserve call identity.

#### Scenario: Prompt contains prior tool history

- **WHEN** prompt history contains a Codex tool call followed by its output
- **THEN** the Chat request contains an assistant message with a matching `tool_calls` entry
- **AND** a following tool message references the same tool call id

#### Scenario: Prompt contains text and a supported image

- **WHEN** a user prompt includes text and a supported image reference (base64 data URL or HTTP(S) URL)
- **THEN** the Chat request encodes that user content as multimodal content parts supported by Chat Completions
- **AND** unsupported image references are represented as explicit text placeholders

#### Scenario: Serialized request is fully tested

- **WHEN** unit tests cover Chat request construction
- **THEN** tests compare complete serialized request objects for text-only, tool-history, and image cases

### Requirement: Chat tools MUST preserve Codex tool identity through the tool loop

Codex MUST serialize available tools into Chat Completions tool declarations without
changing internal `ToolSpec` definitions. Streamed Chat `tool_calls` MUST be reconstructed
into Codex `ResponseItem` values that the existing client-side tool loop can execute.
Namespaced or otherwise Chat-invalid names MUST be sanitized deterministically with
reverse metadata when sanitization is required. The initial phase covers tools serialized as
Chat function tools; reconstruction of custom/freeform, local-shell, and tool-search item
kinds follows the Claude coverage matrix as later work.

#### Scenario: Model streams a function tool call

- **WHEN** Chat Completions streams `delta.tool_calls` for a function tool
- **THEN** Codex emits tool-input deltas while arguments arrive
- **AND** Codex emits a final `ResponseItem` whose kind matches the declared Codex tool kind (initial phase: tools serialized as Chat function tools)
- **AND** the existing Codex tool loop can execute the item and continue the turn

#### Scenario: Multi-tool call indexes remain stable

- **WHEN** a single Chat completion streams multiple tool calls with provider indexes
- **THEN** Codex maps them to stable dense tool indexes for delta association
- **AND** final items do not swap arguments across tool calls

### Requirement: Chat SSE MUST accumulate into Codex ResponseEvent values

Codex MUST parse Chat Completions SSE/chunk streams as a stateful L2 accumulator. The
parser MUST handle assistant text deltas, reasoning/thinking deltas when present,
tool-call deltas, usage, terminal finish reasons, and error payloads. It MUST emit
`ResponseEvent` progressive updates and final items compatible with turn orchestration.

#### Scenario: Chat streams text deltas

- **WHEN** Chat Completions sends multiple content deltas
- **THEN** Codex emits live `OutputTextDelta` events
- **AND** the final message item contains the concatenated text

#### Scenario: Chat streams tool argument fragments

- **WHEN** Chat Completions sends partial `tool_calls` argument fragments
- **THEN** Codex emits `ToolCallInputDelta` events
- **AND** argument JSON is parsed only when a complete tool call is available for item emission

#### Scenario: Chat reports usage and finish reason

- **WHEN** a Chat stream ends with usage and `finish_reason`
- **THEN** Codex emits `Completed` with token usage when available
- **AND** `provider_stop_reason` reflects the Chat finish reason when present

#### Scenario: Chat stream errors are actionable

- **WHEN** Chat Completions returns an HTTP/API error or a malformed stream
- **THEN** Codex surfaces an actionable API/stream error
- **AND** the turn does not silently treat the stream as a successful empty completion

### Requirement: Chat Completions MUST be proven with mocked end-to-end turns

Codex MUST include mocked end-to-end coverage for the Chat Completions boundary that
exercises request path, auth headers, streaming text, tool-call reconstruction, local
tool execution continuation, usage/completion, and error propagation.

#### Scenario: Mocked Chat tool loop

- **WHEN** a mocked Chat endpoint streams a tool call and then accepts a continuation request
- **THEN** Codex executes the client-side tool
- **AND** the next Chat request includes the tool result message(s)
- **AND** the turn can complete with final assistant text

#### Scenario: Mocked Chat path and headers

- **WHEN** a Chat provider turn starts against a mock server
- **THEN** the observed path is the Chat Completions path
- **AND** auth headers match the provider auth contract

