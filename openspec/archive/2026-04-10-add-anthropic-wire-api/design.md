# Design: Anthropic Wire API Support

## Current Architecture

The current architecture already separates several concerns:

- provider configuration: `codex-rs/model-provider-info`
- model metadata discovery: `codex-rs/models-manager`
- protocol-neutral turn orchestration: `codex-rs/core`
- tool specification: `codex-rs/tools`
- generic streamed response events: `codex-rs/codex-api`
- rollout and state persistence: `codex-rs/rollout`, `codex-rs/state`

The main turn path is:

1. `ThreadManager` creates and owns threads
2. `CodexThread` exposes a public thread handle
3. `Session` creates a `TurnContext`
4. `built_tools()` creates a `ToolRouter`
5. `build_prompt()` builds a protocol-neutral `Prompt`
6. `ModelClientSession::stream()` dispatches to the concrete wire protocol
7. streamed events are mapped into `ResponseEvent`
8. `core` handles tool execution, rollout persistence, and UI events

The architecture already has a provider framework, but not yet a complete multi-wire execution framework.

## Problem Statement

The current provider framework handles:

- endpoint base URL
- auth/env/header wiring
- retry and timeout defaults
- provider selection

It does not yet provide a complete abstraction for:

- alternate request bodies
- alternate SSE event models
- alternate tool-use streaming semantics

Today, `WireApi` is effectively single-valued and `ModelClientSession::stream()` only implements Responses.

## Design Goals

- isolate Anthropic implementation in a new crate
- keep almost all new implementation files in one new folder
- keep `core` orchestration unchanged
- preserve existing `ResponseItem` and `ResponseEvent` semantics
- reuse `ToolSpec` and existing tool runtime
- keep the integration boundary small and testable

## Proposed Module Boundary

## New crate: `codex-anthropic`

New crate path:

- `codex-rs/anthropic/`

New crate responsibilities:

- construct Anthropic `MessageCreateParams`
- execute streaming via `anthropic-sdk-rs`
- decode Anthropic stream events
- accumulate partial tool JSON
- map Anthropic protocol events into:
  - `codex_api::ResponseEvent`
  - `codex_protocol::models::ResponseItem`
- map Anthropic SDK errors into `codex_protocol::error::CodexErr`

This crate should depend on:

- `codex-api`
- `codex-model-provider-info`
- `codex-protocol`
- `codex-tools`
- `tokio`
- `serde_json`
- `anthropic-sdk-rs`

It should not depend on `codex-core`.

## Single-Folder Implementation Rule

To reduce upstream conflict risk, all new Anthropic implementation code should be concentrated under:

- `codex-rs/anthropic/`

Allowed exceptions:

- `codex-rs/Cargo.toml`
- `codex-rs/model-provider-info/src/lib.rs`
- `codex-rs/core/src/client.rs`
- config/schema/doc files that must expose the new protocol

Non-exception changes to existing directories should be treated as design failures unless there is a concrete technical reason they cannot be avoided.

## Thin integration boundary

Because `Prompt` currently lives in `codex-core`, the boundary should be a thin transport DTO built in `core` and passed to `codex-anthropic`.

Recommended shape:

- `codex-anthropic` defines an input struct such as `AnthropicTurnRequest`
- `codex-core` converts `Prompt` into `AnthropicTurnRequest`
- `codex-anthropic` returns a `codex_api::ResponseStream` or equivalent stream of `ResponseEvent`

This avoids a `core <-> anthropic` dependency cycle while keeping the request mapping explicit.

## Request Mapping

Map current turn data into Anthropic Messages API as follows:

- `Prompt.base_instructions`
- `developer/system` response items
  - merged into Anthropic `system`
- `user/assistant` response items
  - mapped into Anthropic `messages`
- `ToolSpec`
  - mapped into Anthropic `tools`
- `FunctionCall` / `CustomToolCall` / `LocalShellCall`
  - represented as prior `tool_use`
- `FunctionCallOutput` / `CustomToolCallOutput`
  - represented as `tool_result`

Initial compatibility target:

- `Function`
- `Freeform`
- `LocalShell`

Deferred for later:

- `ImageGeneration`
- full `WebSearch` parity
- image input parity
- Anthropic-specific output-schema auto-repair

## Stream Mapping

Map Anthropic stream events into existing internal events:

- `message_start`
  - `ResponseEvent::Created`
- `text_delta`
  - `ResponseEvent::OutputTextDelta`
- `thinking_delta`
  - `ResponseEvent::ReasoningSummaryDelta` and related reasoning item lifecycle events
- `input_json_delta`
  - accumulate on the active tool block
- `message_stop`
  - emit final assistant `ResponseItem`
  - emit completed tool-call `ResponseItem` values
  - emit `ResponseEvent::Completed`

The mapping target is the existing internal event model. No rollout/state/UI schema changes are required.

## Auth Strategy

Phase 1 should reuse existing provider fields, but be explicit about auth behavior:

- preferred path: `env_key = "ANTHROPIC_API_KEY"`
- supported headers:
  - provider `http_headers`
  - provider `env_http_headers`
- optional support:
  - `experimental_bearer_token` only when the target endpoint accepts bearer auth

Phase 1 should not require command-backed provider auth for official Anthropic API compatibility.

## Minimal Touch Files

Expected integration touch points:

- `codex-rs/Cargo.toml`
  - add workspace member and dependency
- `codex-rs/model-provider-info/src/lib.rs`
  - add `WireApi::Anthropic`
- `codex-rs/core/src/client.rs`
  - add `WireApi::Anthropic` dispatch arm

Everything else should live under `codex-rs/anthropic/` unless an explicit exception is justified.

## Testing Strategy

Use `wiremock` and current core test harnesses to validate:

- plain text streaming
- reasoning stream mapping
- tool use -> tool result -> follow-up assistant response
- auth header behavior
- error mapping

The new crate should also have focused unit tests for:

- request building
- tool spec translation
- partial tool JSON accumulation
- stream event conversion
- auth precedence (`x-api-key` over bearer fallback)

## Explicitly Deferred in This Change

The following remain intentionally outside the current phase:

- `ImageGeneration` tool execution
- `WebSearch` tool execution
- image input handling
- Anthropic-specific output-schema auto-repair
- broader Claude Code compatibility behavior outside the wire protocol

## Alternatives Considered

### Alternative A: put Anthropic implementation directly in `codex-core`

Rejected because it increases upstream conflict surface and continues core growth.

### Alternative B: first extract `Prompt` into a separate common crate

Architecturally cleaner, but larger scope. This may still be a later follow-up if more wire protocols are added.

### Alternative C: rely only on provider configuration

Rejected because provider configuration does not solve request/stream/schema differences between Responses and Anthropic Messages.

## Rollout Plan

1. Add `WireApi::Anthropic`
2. Add new `codex-anthropic` crate
3. Add thin dispatch path from `core`
4. Ship MVP for text, reasoning, and tool use
5. Evaluate follow-up refactor if more protocol adapters are introduced
