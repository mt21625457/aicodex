## Context

Claude Messages and OpenAI Responses both support messages, tool calls, and streaming, but their wire shapes are materially different. Codex should continue to expose one internal runtime model and add a protocol adapter at the provider boundary:

- `codex-tools`: provider-specific tool declaration serialization.
- `codex-api`: provider-specific HTTP request and SSE parsing.
- `codex-core`: provider dispatch and conversion from `Prompt` to provider request.
- Existing tool execution loop: unchanged, driven by `ResponseItem` outputs.

This keeps Claude logic out of the OpenAI Responses parser and avoids forcing the rest of Codex to understand provider-specific content blocks. The same boundary must become the extension point for future wire APIs: a new protocol should add its own adapter pieces and conformance tests without rewriting the turn loop or editing another protocol's parser.

## Goals / Non-Goals

**Goals:**

- Support Claude Messages streaming for Codex turns.
- Preserve existing client-side tool loops for function calls, custom/freeform tools, local shell, and tool search.
- Establish a reusable Wire API adapter abstraction that future provider protocols can implement.
- Support input history with text, base64 images, and remote HTTP(S) image URLs.
- Parse Claude stream events for text, tool input deltas, thinking/signature deltas, usage, terminal completion, and error events.
- Prove the implementation with mocked `/v1/messages` end-to-end tests.

**Non-Goals:**

- Do not add `anthropic-rs` as a dependency.
- Do not implement Claude batches, models list, count-tokens, documents, or prompt caching in this change.
- Do not design a speculative plugin framework for protocols that are not being implemented now; generalize only the boundaries proven by OpenAI Responses and Claude Messages.
- Do not change the public internal `ResponseItem` model solely to mirror Claude content blocks.
- Do not move tool execution into a Claude-specific loop; Codex's existing loop remains the authority.

## Reference Comparison: `anthropic-rs`

| Area | `anthropic-rs` behavior | Codex adaptation |
| --- | --- | --- |
| Request types | `MessagesRequest` owns `model`, `messages`, `max_tokens`, optional `system`, `tools`, `tool_choice`, `thinking`, and `stream`. Content is typed as `ContentBlock`. | Add local Claude request/content structs at the Codex wire boundary. Keep `serde_json::Value` only for tool schemas and model-provided tool input. |
| System/history | `SystemPrompt` is separate from `messages`; messages have only `user` or `assistant` roles. | Move base/developer/system instructions into Claude `system`; convert Codex user/assistant messages to content blocks; reject or normalize invalid history before send. |
| Images | Inline base64 images are `{"type":"image","source":{"type":"base64","media_type","data"}}`; remote images use `{"type":"url","url"}`. | Support base64 data URLs and HTTP(S) image URLs as native Claude image blocks; degrade unsupported image references to explicit text placeholders. |
| Tool results | Tool results accept plain text or structured content blocks. | Preserve multimodal Codex tool outputs as Claude `tool_result.content` blocks when available, falling back to text for legacy outputs. |
| Tool declarations | `Tool` has `name`, `description`, and `input_schema`; `ToolChoice` supports `auto`, `any`, specific tool, and none. | Serialize Codex tools to Claude tools, flatten namespaces into Claude-valid names, and preserve a side table for reverse mapping. |
| Tool loop | On `tool_use`, append assistant response content, execute locally, then append a user `tool_result` message. | Claude SSE parser emits Codex `FunctionCall`, `CustomToolCall`, `LocalShellCall`/function-equivalent, and `ToolSearchCall`; the existing Codex loop sends outputs back as Claude `tool_result` blocks on the next request. |
| Stream deltas | `StreamAccumulator` accumulates text, partial JSON, thinking, signature, stop reason/sequence, and usage. | Codex parser should accumulate per content block and emit live `ResponseEvent` deltas plus final `ResponseItemDone` items. `input_json_delta` is parsed at `content_block_stop`. |
| Stream error | SSE `event: error` or `type: error` becomes a typed API error. | Surface Claude stream errors as `ApiError::Stream`/mapped provider error without silently continuing. |
| Headers/path | `POST /v1/messages`, `x-api-key`, `anthropic-version`, JSON content, event stream. | Provider base URL should end at `/v1`; endpoint path is `messages`; auth provider must emit `x-api-key`; adapter supplies default `anthropic-version: 2023-06-01` and `Accept: text/event-stream`. |

Follow-up comparison against the local `anthropic-rs` SDK identified two
additional adapter gaps worth closing in this change:

- `MessagesRequest` supports `thinking` and `service_tier`; Codex maps
  configured reasoning effort and service tier into those Claude request fields.
- `StreamAccumulator` rejects malformed stream ordering and mismatched deltas;
  Codex now treats content block events before `message_start`, unknown block
  indexes, and mismatched delta kinds as stream errors instead of accepting them
  leniently.
- Direct wire-shape tests now cover the Claude content block variants Codex
  emits, Anthropic `tool_choice` variants, thinking enabled/disabled
  serialization, service tier serialization, cache usage accounting, and
  multiple tool-use/tool-result ordering in the Codex tool loop.
- The request adapter explicitly covers the image cases Codex supports:
  base64 data URLs, HTTP(S) URL image blocks, structured tool-result image
  blocks, and unsupported image references represented as text placeholders.
- Claude HTTP error responses that use Anthropic's JSON error envelope are now
  mapped at the Claude Messages endpoint boundary into actionable Codex API
  errors, including rate-limit and overloaded classifications, keeping that
  provider-specific parsing out of shared transport and OpenAI response parsing.
- Mock end-to-end coverage now proves both successful and failed client-side
  tool execution continuations: failed tools are sent back to Claude as
  `tool_result` blocks with `is_error: true`.

## Decisions

### 1. Define a reusable Wire API adapter contract

The implementation should converge on a small protocol adapter contract with these extension points:

- provider endpoint and header policy,
- request/history conversion from `Prompt` and `ModelInfo`,
- provider-specific tool declaration serialization plus reverse mapping metadata,
- streaming parser/accumulator that emits Codex `ResponseEvent`s,
- mock conformance tests for request path, headers, stream deltas, tool calls, tool outputs, usage, and errors.

Core may dispatch by `WireApi`, but provider-specific details should live in adapter modules. Adding a future protocol should primarily require a new `WireApi` variant, a new adapter module, provider metadata/auth policy, and conformance tests.

### 2. Use `WireApi::Claude` as the routing boundary

Provider metadata decides whether a turn uses OpenAI Responses or Claude Messages. Core dispatch may select the Claude request builder and Claude Messages client, but it must not branch inside the Responses request body or SSE parser.

### 3. Use typed Claude wire structs inside Codex

The request adapter should define typed structs/enums for Claude messages and content blocks. This prevents accidentally emitting invalid block shapes and makes tests compare whole serialized objects. Tool input and `input_schema` remain `serde_json::Value` because their schemas and model-produced arguments are open-ended JSON by design.

### 4. Preserve Claude history order strictly

Claude `messages` must contain only `user` and `assistant` roles. System/developer/base instructions must be merged into the top-level `system` field. Tool history must be represented as:

1. Assistant message containing one or more `tool_use` blocks.
2. Following user message containing corresponding `tool_result` blocks.

Adjacent same-role messages may be coalesced only when doing so cannot move a `tool_result` before its `tool_use` or hide an assistant turn boundary that Claude requires.

### 5. Parse streams as an accumulator, not one-off event mapping

Claude's `input_json_delta` arrives in fragments and is not valid JSON until the content block stops. The parser must maintain state per content block index, accumulate text/thinking/tool JSON, merge usage from `message_start` and `message_delta`, and emit final Codex items only after terminal state is known.

### 6. Treat tool name mapping as part of the wire contract

Claude tool names must satisfy Anthropic naming constraints. Namespaced MCP tools and Codex custom tools should be flattened and sanitized deterministically. The reverse mapping side table must carry the original name, namespace, and Codex tool kind so streamed `tool_use.name` can reconstruct the correct `ResponseItem`.

### 7. Verify with mock end-to-end tests

Unit tests for serializers are necessary but insufficient. The change must also mount a mock Claude Messages endpoint and run a Codex turn through:

- request construction,
- `/v1/messages` routing,
- `x-api-key` and `anthropic-version` headers,
- SSE `tool_use`,
- local tool output,
- second request containing Claude `tool_result`,
- final assistant text and usage completion.

## Risks / Trade-offs

- Typed wire structs add more code than direct `serde_json::Value`, but they catch invalid protocol shapes earlier and match the stability needed for a provider adapter.
- A reusable adapter contract adds a little up-front structure, but it prevents future protocols from copying Claude-specific branches through core and keeps protocol behavior testable in isolation.
- Claude does not have Responses namespaces or freeform custom tools, so flattening/sanitizing names introduces collision risk. The adapter must use deterministic suffixing or collision detection and test long/invalid names.
- Mapping local shell through Claude tools exposes local shell args as model-visible JSON. The schema must stay narrow and must not bypass existing sandbox/approval behavior.
- Extended thinking may contain signature blocks that are not user-visible text. The parser must preserve the signature when needed for protocol correctness without leaking it into ordinary output text.

## Verification

- `cd codex-rs && just fmt`
- `cd codex-rs && cargo test -p codex-tools claude`
- `cd codex-rs && cargo test -p codex-api claude`
- `cd codex-rs && cargo test -p codex-model-provider-info -p codex-model-provider claude`
- `cd codex-rs && cargo test -p codex-config claude`
- `cd codex-rs && cargo test -p codex-core claude`
- `cd codex-rs && just fix -p codex-api`
- `cd codex-rs && just fix -p codex-tools`
- `cd codex-rs && just fix -p codex-core`

If config schema changes, also run:

- `cd codex-rs && just write-config-schema`

If Rust dependencies change, also run from the repository root:

- `just bazel-lock-update`
- `just bazel-lock-check`
