## Why

Codex currently has an OpenAI Responses-centered provider path, while Claude requires the Anthropic Messages wire protocol: request history is expressed as alternating `user` / `assistant` messages, system instructions live outside `messages`, tool calls are `tool_use` content blocks, and tool outputs are `tool_result` user blocks. Treating Claude as a shallow variant of Responses risks invalid request history, incomplete tool-call reconstruction, and stream events that cannot round-trip through the existing Codex tool loop.

The local reference implementation at `/Users/mt/code/mt-ai/aicodex/anthropic-rs` shows the protocol surfaces that must be handled explicitly:

- `anthropic/src/types.rs` models typed Messages requests/responses, content blocks, `tool_use`, `tool_result`, thinking blocks, usage, and stream event enums.
- `anthropic/src/stream.rs` uses a stream accumulator that folds `text_delta`, `input_json_delta`, `thinking_delta`, `signature_delta`, `message_delta` usage, and terminal events into a complete response.
- `anthropic/src/client.rs` sends `POST /v1/messages` with `x-api-key`, `anthropic-version: 2023-06-01`, JSON content, and event-stream handling.
- `anthropic/src/tool_loop.rs` demonstrates the transcript shape for tool loops: append the assistant `tool_use` turn, execute tools locally, then append a user `tool_result` turn.

Codex should not depend on or embed that crate. The change should implement the same wire semantics inside Codex's adapter layer, preserving Codex's existing `ResponseItem` / `ResponseEvent` model and client-side tool execution.

This is also the first substantial non-OpenAI Responses provider path. The implementation must establish a reusable Wire API adapter abstraction rather than a Claude-only side path, so future protocols can add request serialization, tool serialization, auth/header policy, and stream accumulation behind the same core dispatch boundary.

## What Changes

- Add Claude as a first-class `WireApi` route, with providers using Claude Messages over `POST /v1/messages` instead of OpenAI Responses.
- Introduce a reusable Wire API adapter contract that separates provider dispatch from protocol-specific request building, tool declaration serialization, auth/header policy, and stream parsing.
- Add a typed Claude Messages request/history adapter in `codex-api` / `codex-core` that builds valid Messages payloads from Codex `Prompt` data.
- Add Claude-compatible tool serialization in `codex-tools`, including stable name flattening and side-table metadata to reconstruct Codex function/custom/local shell/tool-search calls from Claude `tool_use` blocks.
- Add a Claude SSE parser/accumulator in `codex-api` that translates Claude stream events into Codex `ResponseEvent`s and final `ResponseItem`s.
- Add Anthropic auth support that emits `x-api-key` and default `anthropic-version`, while staying compatible with the existing provider/auth abstractions.
- Add mock end-to-end coverage proving the request path, auth headers, streaming deltas, usage, errors, and tool loop all work through the Codex runtime.

## Capabilities

### New Capabilities

- `claude-wire-api-support`: Codex can route selected model providers through Anthropic Claude Messages while preserving internal turn/tool-loop behavior.
- `wire-api-adapter-abstraction`: Codex has a protocol adapter boundary that future wire APIs can implement without modifying existing OpenAI or Claude protocol internals.
- `claude-message-history-adapter`: Codex can translate internal prompt history into valid Claude `system` and `messages` fields, including text, base64/URL images, `tool_use`, and `tool_result` blocks with structured content when available.
- `claude-stream-accumulator`: Codex can parse Claude Messages SSE and emit equivalent Codex streaming events for text, reasoning, tool input, usage, completion, and errors.

### Modified Capabilities

- `model-provider-routing`: Provider configuration can select `WireApi::Claude`.
- `tool-wire-serialization`: Tool declarations can be serialized for both OpenAI Responses and Claude Messages without changing internal tool definitions.
- `provider-authentication`: Anthropic API keys use the Claude header contract instead of bearer-token Responses headers.

## Impact

- Affected crates:
  - `codex-rs/codex-api`
  - `codex-rs/tools`
  - `codex-rs/core`
  - `codex-rs/model-provider-info`
  - `codex-rs/model-provider`
  - `codex-rs/config`
- Affected docs:
  - `docs/config.md`
  - `codex-rs/core/config.schema.json` when provider config schema changes
- Primary risks:
  - Invalid Claude history if adjacent tool/use result turns are merged incorrectly.
  - Lost or malformed tool input if `input_json_delta` chunks are not accumulated and parsed at block stop.
  - Missing reasoning content if `thinking_delta` / `signature_delta` are ignored.
  - Auth regressions if Anthropic requests use bearer headers instead of `x-api-key`.
  - Future protocol work becomes expensive if this change hard-codes Claude behavior into core dispatch instead of a reusable adapter contract.
  - False confidence if tests only inspect unit adapters and do not run a full mocked tool loop.
- Rollback:
  - Revert `WireApi::Claude` provider selection and Claude-specific adapter modules while leaving existing OpenAI Responses behavior untouched.
