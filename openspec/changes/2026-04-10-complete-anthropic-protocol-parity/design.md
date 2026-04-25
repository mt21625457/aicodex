# Design: Complete Anthropic Protocol Parity and Tool Coverage

## Overview

This change upgrades the Anthropic adapter from a minimal phase-1 bridge to a protocol-faithful implementation for multimodal and tool-driven turns.

The core design principle remains unchanged:

- `codex-core` orchestrates turns and executes tools
- `codex-anthropic` translates between Anthropic Messages semantics and Codex's internal event/item model

## Design Goals

- Preserve Anthropic protocol details that must survive across turns
- Reuse existing internal tool handlers
- Avoid silent degradation of content or capability
- Keep unsupported features explicit

## 1. Thinking continuity

Anthropic tool loops require the last assistant thinking blocks to be replayed intact.

### Design

- Extend the Anthropic DTO to accept turn-level reasoning controls
- When Anthropic emits thinking blocks:
  - preserve `thinking` / `redacted_thinking`
  - preserve signature data
- Store the raw Anthropic thinking block sequence in a serialized envelope carried by `ResponseItem::Reasoning.encrypted_content`
- When building a follow-up Anthropic request:
  - decode that envelope
  - replay the raw thinking blocks as part of the assistant turn before replaying `tool_use`

### Why this shape

This avoids widening `ResponseItem` with Anthropic-specific variants while still keeping the raw provider payload available for protocol-faithful replay.

## 2. Structured and multimodal tool results

Anthropic `tool_result.content` may be either:

- a plain string
- a list of content blocks such as text and image blocks

### Design

- Map `FunctionCallOutputPayload::Text` to plain-string tool result content
- Map `FunctionCallOutputPayload::ContentItems` to Anthropic content blocks
- Support image results from existing tools such as `view_image`
- Reject unsupported image source shapes explicitly

## 3. Multimodal user input

Anthropic supports image content blocks in user messages.

### Design

- Convert `ContentItem::InputImage` into Anthropic image blocks
- Preserve mixed image + text ordering within the same user message
- Reject image content only in places Anthropic itself does not allow, such as system text assembly

## 4. Tool-use completion boundary

Anthropic sends tool input incrementally via `input_json_delta`.

### Design

- Accumulate partial JSON until `content_block_stop`
- Do not convert a tool-use block into a Codex `ResponseItem` before the block completes
- Treat parse failure or missing required tool-use metadata as protocol errors
- If a tool-use block is interrupted by `max_tokens`, surface an incomplete-turn error rather than executing a partial tool call

## 5. Stop reasons and stream errors

Anthropic stop reasons must not all be treated as successful completion.

### Design

- Handle at least:
  - `end_turn`
  - `tool_use`
  - `max_tokens`
  - `pause_turn`
  - `refusal`
  - `model_context_window_exceeded`
- Map SSE `event:error` payloads such as `overloaded_error` into meaningful `CodexErr` variants
- Preserve retryability where the upstream error semantics justify it

## 6. Provider-aware tool exposure

Unsupported tools should be filtered before request construction rather than disappearing silently inside the Anthropic adapter.

### Design

- Introduce provider-aware tool filtering in core prompt/tool selection
- Anthropic-exposed tools should be the subset that the adapter can faithfully:
  - describe
  - parse from `tool_use`
  - replay in history
  - return as `tool_result`
- Adapter-level explicit errors remain for historical items that still reach unsupported paths

## 7. Tool capability rollout

### `web_search`

Preferred first expansion target because:

- the tool already exists in Codex
- request/response payloads are structurally representable as Anthropic tool use

### `tool_search`

Evaluate whether it should be:

- exposed as an Anthropic tool, or
- filtered for Anthropic if it remains an internal helper shape

### `image_generation`

Requires both:

- request-side tool mapping
- response-side image result mapping

This is feasible but should remain isolated to a separate task slice if it risks delaying the protocol correctness work.

## 8. Per-turn controls

### `service_tier`

Map existing Codex `ServiceTier` values onto Anthropic request semantics using the request `extra` field.

### `turn_metadata_header`

Two acceptable outcomes:

- leave it internal-only and document that Anthropic has no equivalent request field
- forward it as a custom per-request header for observability

It should not block correctness work.

## 9. Testing

### Unit tests in `codex-anthropic`

- thinking request controls
- thinking block replay
- structured/image tool results
- image input
- incomplete tool-use rejection
- stop-reason handling
- overloaded SSE error mapping
- cache token accounting

### Core integration tests

- Anthropic text/reasoning/tool round trips
- multimodal turn construction
- provider-aware tool exposure behavior
- service tier request serialization

## 10. Documentation

Update Anthropic docs to distinguish:

- supported today
- filtered/unsupported today
- supported only after provider-aware tool exposure lands
