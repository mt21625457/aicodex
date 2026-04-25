## ADDED Requirements

### Requirement: Wire API adapters must be reusable for future protocols

Codex MUST introduce a protocol adapter boundary that keeps provider-specific wire behavior out of the shared turn loop. A wire API adapter MUST own endpoint/header policy, prompt-to-request conversion, tool declaration serialization, reverse tool-call mapping, stream accumulation, and protocol-specific mock conformance tests. Adding a future protocol MUST NOT require modifying the OpenAI Responses parser or Claude Messages parser except through shared abstractions that are already provider-neutral.

#### Scenario: A future protocol is added after Claude

- **WHEN** a new provider protocol is introduced after this change
- **THEN** the implementation can add a new `WireApi` route and protocol adapter module
- **AND** the shared Codex tool loop continues to consume and produce `ResponseItem` values
- **AND** existing OpenAI Responses and Claude Messages parser internals do not need protocol-specific edits

#### Scenario: Adapter conformance is tested

- **WHEN** a wire API adapter is implemented
- **THEN** tests cover request path/header policy, request history conversion, stream text deltas, tool-call reconstruction, tool-result continuation, usage reporting, and error propagation

### Requirement: Claude providers must use the Claude Messages wire API

Codex MUST route providers configured with `WireApi::Claude` to the Claude Messages endpoint rather than the OpenAI Responses endpoint. The request path MUST resolve to `/v1/messages` when using the default Anthropic base URL. Claude requests MUST include Anthropic authentication and version headers, including `x-api-key` and `anthropic-version: 2023-06-01`.

#### Scenario: Claude provider sends a streaming request

- **WHEN** a model provider is configured with `WireApi::Claude`
- **THEN** Codex sends a streaming request to `/v1/messages`
- **AND** the request includes `Accept: text/event-stream`
- **AND** the request includes `x-api-key` with the configured Anthropic credential
- **AND** the request does not use the OpenAI Responses request path

### Requirement: Claude history must be serialized as valid Messages API history

Codex MUST translate internal prompt history into Claude Messages API structure. System, developer, and base instructions MUST be placed in the top-level `system` field. `messages` MUST contain only `user` and `assistant` roles. Text content MUST serialize as `text` blocks. Base64 data URL images MUST serialize as Claude base64 `image` blocks. HTTP(S) image URLs MUST serialize as Claude URL `image` blocks. Function/custom/local-shell/tool-search calls MUST serialize as assistant `tool_use` blocks, and tool outputs MUST serialize as following user `tool_result` blocks. Structured multimodal tool outputs MUST be preserved as Claude `tool_result.content` blocks when possible.

#### Scenario: Prompt contains text and a base64 image

- **WHEN** a user prompt includes input text and a base64 data URL image
- **THEN** the Claude request contains one `user` message
- **AND** that message contains a `text` block and an `image` block with `source.type = "base64"`

#### Scenario: Prompt contains an HTTP image URL

- **WHEN** a user prompt includes an HTTP(S) image URL
- **THEN** the Claude request contains an `image` block with `source.type = "url"`
- **AND** unsupported non-HTTP image references are represented as explicit text placeholders

#### Scenario: Prompt contains prior tool history

- **WHEN** prompt history contains a Codex tool call followed by its output
- **THEN** the Claude request contains an `assistant` message with a matching `tool_use` block
- **AND** the following `user` message contains a `tool_result` block whose `tool_use_id` matches the original tool call id

#### Scenario: Tool output contains structured content

- **WHEN** a Codex tool output contains text and image content items
- **THEN** the Claude request sends a following `user` message with `tool_result.content` as content blocks
- **AND** image content items use Claude base64 or URL image sources where supported
- **AND** unsupported image references are represented as explicit text placeholders

#### Scenario: Claude request includes reasoning and service settings

- **WHEN** a Claude turn has a configured Codex reasoning effort
- **THEN** the Claude request includes an Anthropic `thinking` configuration with a bounded token budget
- **AND** supported Codex service tier settings are translated to Claude `service_tier` values

### Requirement: Claude tools must preserve Codex tool identity

Codex MUST serialize available tools as Claude `tools` entries with valid Claude tool names and JSON input schemas. If a Codex tool has a namespace or a name that is not valid for Claude, Codex MUST produce a deterministic Claude-safe name and keep reverse metadata that maps streamed `tool_use.name` values back to the original Codex namespace, tool name, and tool kind.

Codex's Claude request wire types MUST support Anthropic `tool_choice` variants for automatic selection, any-tool selection, named-tool selection, and no-tool selection. The turn adapter MAY emit only the subset needed by current Codex behavior.

#### Scenario: Namespaced MCP tool is called by Claude

- **WHEN** Claude streams a `tool_use` block for a flattened MCP tool name
- **THEN** Codex emits a `FunctionCall` `ResponseItem`
- **AND** the emitted item preserves the original namespace and function name

#### Scenario: Custom/freeform tool is called by Claude

- **WHEN** Claude streams a `tool_use` block for a Codex custom/freeform tool
- **THEN** Codex emits a `CustomToolCall` `ResponseItem`
- **AND** streamed input deltas are exposed as tool call input deltas where applicable

#### Scenario: Claude tool choice variants serialize correctly

- **WHEN** a Claude request uses `auto`, `any`, named `tool`, or `none` tool choice
- **THEN** the adapter serializes the Anthropic `tool_choice` discriminant without falling back to untyped JSON

### Requirement: Claude SSE must be accumulated into Codex response events

Codex MUST parse Claude Messages SSE as a stateful stream. The parser MUST handle `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, and `error` events. It MUST accumulate text, tool input JSON fragments, thinking text, signature deltas, stop metadata, and usage before emitting final Codex response items and completion.

Claude HTTP error responses that use Anthropic's JSON error envelope MUST be surfaced as actionable Codex API errors at the Claude Messages endpoint boundary.

#### Scenario: Claude streams text deltas

- **WHEN** Claude sends a text content block followed by multiple `text_delta` events
- **THEN** Codex emits live output text deltas
- **AND** the final message item contains the concatenated text

#### Scenario: Claude streams tool input JSON fragments

- **WHEN** Claude sends a `tool_use` block with multiple `input_json_delta` fragments
- **THEN** Codex buffers the fragments by content block index
- **AND** Codex parses the final JSON at `content_block_stop`
- **AND** the final tool call item contains the complete parsed input

#### Scenario: Claude streams extended thinking

- **WHEN** Claude sends `thinking_delta` and `signature_delta` events
- **THEN** Codex emits reasoning content for thinking text
- **AND** signature data is preserved for protocol handling without being appended to ordinary assistant text

#### Scenario: Claude sends usage and completion

- **WHEN** Claude sends `message_delta` usage and then `message_stop`
- **THEN** Codex emits a completed response event with merged token usage
- **AND** cache creation/read token fields are reflected in cached input accounting where supported

#### Scenario: Claude sends a stream error

- **WHEN** Claude sends an SSE error event or a payload with `type = "error"`
- **THEN** Codex surfaces an API error
- **AND** Codex does not emit a successful completed response for that stream

#### Scenario: Claude sends an HTTP API error

- **WHEN** the Claude Messages HTTP endpoint returns a non-success response with an Anthropic error envelope
- **THEN** Codex surfaces the Anthropic error type and message as an API error
- **AND** Anthropic rate-limit and overloaded error types map to Codex rate-limit and server-overloaded errors
- **AND** this error mapping is implemented in the Claude Messages endpoint adapter

#### Scenario: Claude sends malformed stream ordering

- **WHEN** Claude sends a content block delta before `message_start`
- **OR** Claude sends a delta for an unknown content block index
- **OR** Claude sends a delta whose type does not match the started content block
- **THEN** Codex surfaces an API error
- **AND** Codex does not synthesize invalid response items from the malformed stream

### Requirement: Mock end-to-end tests must prove the Claude tool loop

Codex MUST include mock end-to-end tests that drive a Claude Messages turn through provider dispatch, request serialization, streaming tool use, client-side tool execution, tool result serialization, and final assistant completion.

#### Scenario: Claude calls a client-side tool and receives the result

- **WHEN** the mocked Claude endpoint first streams a `tool_use` response
- **AND** Codex executes the corresponding client-side tool
- **THEN** the next mocked Claude request contains a user `tool_result` block with the same tool id
- **AND** the final mocked response is emitted as normal assistant text

#### Scenario: Claude receives a client-side tool error

- **WHEN** the mocked Claude endpoint streams a `tool_use` response whose local execution returns an error
- **THEN** the next mocked Claude request contains a user `tool_result` block with the same tool id
- **AND** that `tool_result` block includes `is_error = true`

#### Scenario: Claude request headers are verified in end-to-end coverage

- **WHEN** the mocked Claude endpoint receives a Codex request
- **THEN** the test asserts `/v1/messages`, `x-api-key`, `anthropic-version`, and event-stream headers before returning SSE data
