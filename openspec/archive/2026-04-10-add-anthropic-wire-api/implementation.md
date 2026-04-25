# Implementation Plan: Anthropic Wire API v1

## Intent

This document turns the approved design into a concrete implementation layout.

Primary constraint:

- almost all new code lives in `codex-rs/anthropic/`

Allowed integration edits outside that folder remain intentionally narrow.

## New Workspace Layout

Create a new workspace member:

- `codex-rs/anthropic/`

Recommended initial layout:

```text
codex-rs/anthropic/
├── Cargo.toml
└── src/
    ├── lib.rs
    ├── auth.rs
    ├── dto.rs
    ├── error.rs
    ├── request.rs
    ├── stream.rs
    ├── tool_mapping.rs
    └── tests.rs
```

Optional later split if needed:

```text
src/
├── stream/
│   ├── mod.rs
│   ├── state.rs
│   └── mapping.rs
```

But phase 1 should prefer fewer files until complexity justifies extraction.

## Ownership by File

### `src/lib.rs`

Public crate entrypoints only.

Recommended exports:

- `stream_anthropic(...)`
- `AnthropicTurnRequest`

This file should stay thin.

### `src/dto.rs`

Defines the thin integration DTO passed from `codex-core` into `codex-anthropic`.

Recommended shape:

```rust
pub struct AnthropicTurnRequest {
    pub provider: ModelProviderInfo,
    pub model: String,
    pub input: Vec<ResponseItem>,
    pub tools: Vec<ToolSpec>,
    pub parallel_tool_calls: bool,
    pub base_instructions: BaseInstructions,
    pub output_schema: Option<serde_json::Value>,
}
```

This avoids a direct dependency on `codex_core::Prompt`.

### `src/auth.rs`

Responsible for translating current provider config into Anthropic SDK client credentials and headers.

Scope:

- `env_key`
- `experimental_bearer_token`
- provider static/env headers
- timeout/retry extraction

Phase 1 recommendation:

- optimize for `ANTHROPIC_API_KEY`
- do not expand auth surface beyond what is needed for stable MVP

### `src/request.rs`

Responsible for turning `AnthropicTurnRequest` into Anthropic SDK request types.

Main duties:

- merge base instructions and model-visible dev/system content into Anthropic `system`
- map user/assistant message history to Anthropic `messages`
- map prior tool calls and tool results
- attach `tools`
- attach output-schema instructions when present

### `src/tool_mapping.rs`

Protocol-specific translation between internal `ToolSpec` and Anthropic tool schema.

Phase 1:

- `ToolSpec::Function`
- `ToolSpec::Freeform`
- `ToolSpec::LocalShell`

Phase 1 explicit deferrals:

- `ImageGeneration`
- complete `WebSearch` parity
- any future protocol-specific tool families

### `src/stream.rs`

Owns the runtime stream loop and state machine.

Responsibilities:

- call `anthropic-sdk-rs`
- read streaming events
- maintain partial stream state
- emit internal `ResponseEvent`
- finalize assistant/tool/reasoning items on `message_stop`

Recommended internal state:

- current response id
- current assistant text buffer
- current reasoning buffers
- current tool-use buffers keyed by block index
- stop reason
- token usage snapshot

### `src/error.rs`

Maps Anthropic SDK errors into existing `CodexErr` values.

Keep this local to the crate so protocol-specific quirks do not leak into unrelated code.

### `src/tests.rs`

Focused unit tests for:

- request building
- auth/header selection
- tool schema translation
- stream event mapping
- partial tool JSON accumulation

## Minimal Integration Changes Outside `codex-rs/anthropic/`

## `codex-rs/Cargo.toml`

Required changes:

- add workspace member `anthropic`
- add workspace dependency `anthropic-sdk-rs`
- add workspace dependency `codex-anthropic`

## `codex-rs/model-provider-info/src/lib.rs`

Required changes:

- add `WireApi::Anthropic`
- support serde parse/display for `"anthropic"`

This is a required exception because provider framework ownership already lives here.

## `codex-rs/core/src/client.rs`

Required changes:

- add a new `WireApi::Anthropic` match arm in `ModelClientSession::stream()`
- build `AnthropicTurnRequest`
- delegate to `codex_anthropic::stream_anthropic(...)`

This must remain thin.

Forbidden direction:

- do not copy Anthropic request-building logic into `core/src/client.rs`
- do not add Anthropic stream state machines under `core/src/`

## Optional docs/config touch points

Only if needed for exposure:

- provider config docs
- generated config schema
- tests asserting config parsing

## Concrete MVP Integration Contract

Recommended `codex-anthropic` entrypoint:

```rust
pub async fn stream_anthropic(
    request: AnthropicTurnRequest,
) -> codex_protocol::error::Result<codex_api::ResponseStream>
```

Why this shape:

- keeps `core` ignorant of Anthropic SDK details
- keeps `codex-anthropic` independent of `codex-core`
- reuses existing `codex_api::ResponseEvent` stream type

## Mapping Rules

### Internal input to Anthropic request

- `ResponseItem::Message(role=system|developer)` -> append to Anthropic `system`
- `ResponseItem::Message(role=user|assistant)` -> Anthropic message blocks
- `ResponseItem::FunctionCall` -> assistant `tool_use`
- `ResponseItem::FunctionCallOutput` -> user `tool_result`
- `ResponseItem::CustomToolCall` -> assistant `tool_use`
- `ResponseItem::CustomToolCallOutput` -> user `tool_result`
- `ResponseItem::LocalShellCall` -> assistant `tool_use`

### Anthropic stream to internal output

- `message_start` -> `ResponseEvent::Created`
- `text_delta` -> `ResponseEvent::OutputTextDelta`
- `thinking_delta` -> reasoning lifecycle events
- `input_json_delta` -> accumulate partial tool JSON
- `message_stop` -> emit completed assistant and tool items, then `Completed`

## Recommended Phase Order

### Phase 1A: Plumbing

- workspace member
- dependency wiring
- `WireApi::Anthropic`
- empty adapter crate with compile-only integration

### Phase 1B: Text path

- request mapping for text-only turns
- response streaming for text-only turns
- error mapping

### Phase 1C: Tool path

- tool schema mapping
- tool use round trip
- partial JSON accumulation

### Phase 1D: Reasoning path

- thinking/reasoning streaming mapping

### Phase 1E: Docs and config exposure

- config examples
- schema updates
- tests for config parsing

## Files That Should Not Change in Phase 1

To keep merge-conflict surface low, phase 1 should avoid unrelated edits in:

- `codex-rs/core/src/codex.rs`
- `codex-rs/core/src/tools/`
- `codex-rs/rollout/`
- `codex-rs/state/`
- `codex-rs/tui/`
- `codex-rs/app-server/`

If a change seems to require those files, treat that as a design review trigger.

## Acceptance Signal

The implementation is architecturally acceptable when:

- all new Anthropic logic lives under `codex-rs/anthropic/`
- only thin required wiring changes exist outside that folder
- a turn can select `wire_api = "anthropic"`
- streamed Anthropic responses drive the existing tool/runtime/persistence pipeline without a parallel execution stack
