# Proposal: Complete Anthropic Protocol Parity and Tool Coverage

## Summary

Promote the Anthropic Messages API integration from a phase-1 subset into a stable provider path with protocol-faithful thinking/tool loops, explicit capability boundaries, and broader tool coverage.

This change will:

- complete the Anthropic thinking/tool loop so assistant thinking blocks can round-trip across tool calls
- support structured and image-bearing `tool_result` payloads instead of degrading them to plain text
- support Anthropic image input blocks for user turns
- implement Anthropic-specific stop-reason handling and stream error classification
- use provider-aware tool exposure so unsupported tools are filtered before Anthropic requests are built
- add Anthropic support for existing Codex tool implementations where feasible, starting with `web_search`
- map Anthropic `service_tier` using existing Codex per-turn controls
- document unsupported capabilities explicitly when parity is still not possible

## Motivation

The current Anthropic adapter is usable for basic text and some tool flows, but it still has protocol gaps that affect correctness and stability:

- thinking deltas are not enough by themselves; Anthropic expects the last assistant thinking blocks to be returned intact across tool loops
- tool results may carry structured text and images, but plain-text coercion loses information and breaks multimodal flows
- partial `input_json_delta` fragments must not be executed as complete tool calls
- Anthropic stop reasons such as `max_tokens`, `pause_turn`, and `model_context_window_exceeded` need explicit handling
- unsupported tools should not silently disappear at the provider adapter layer

Without these fixes, Claude support remains vulnerable to silent correctness bugs and fragile behavior on longer or more multimodal turns.

## Goals

- Preserve Anthropic protocol semantics for:
  - thinking blocks
  - tool use / tool result loops
  - structured and multimodal content
  - stop reasons and stream-side error events
- Expose only tools that Anthropic can faithfully execute for the current turn
- Reuse existing Codex tool handlers instead of inventing provider-specific tool runtimes
- Keep Anthropic-specific logic concentrated in `codex-rs/anthropic/` where possible
- Surface unsupported capabilities explicitly rather than degrading them silently

## Non-Goals

- Do not redesign the global tool runtime architecture
- Do not create a general cross-provider capability negotiation framework in this change
- Do not promise full parity for Anthropic server tools if Codex does not have an equivalent local execution path
- Do not introduce provider-specific product behavior in the TUI beyond what is required for correctness and telemetry

## Scope

### Protocol correctness

- Add Anthropic thinking request controls
- Preserve raw thinking blocks and signatures across tool loops
- Encode/decode structured `tool_result` content, including images
- Support Anthropic image input blocks
- Enforce content-block completion before executing tool calls

### Stability

- Map stream-side `event:error` payloads, including `overloaded_error`
- Handle Anthropic stop reasons explicitly
- Respect provider stream retry budget where the transport can safely recover
- Improve token-usage accounting for Anthropic cache fields

### Tool coverage

- Filter unsupported tools before Anthropic request construction
- Add Anthropic-facing schema + round-trip support for existing Codex tools where protocol mapping is straightforward
- Start with `web_search`
- Evaluate `tool_search` and `image_generation` separately, documenting any remaining gaps

### Per-turn controls

- Map `service_tier` to Anthropic request semantics
- Decide whether `turn_metadata_header` should remain internal-only or be forwarded as a custom header for gateway observability

## Proposed Shape

### Anthropic request/stream adapter

`codex-rs/anthropic/` remains the home for:

- request construction
- content block mapping
- tool schema mapping
- stream decoding
- stop-reason/error mapping

### Core integration

`codex-core` continues to own:

- tool registration
- tool execution
- session retry loop
- persistence and rollout handling

Core changes should focus on:

- provider-aware tool exposure
- passing through additional turn controls needed by Anthropic

## Expected Impacted Areas

Primary implementation:

- `codex-rs/anthropic/`

Thin integration:

- `codex-rs/core/src/client.rs`
- tool registration / prompt-building code in `codex-rs/core/`

Tests:

- `codex-rs/anthropic/src/tests.rs`
- `codex-rs/core/tests/suite/anthropic.rs`

Docs:

- `docs/config.md`
- `doc/anthropic-manual-test-config.md`

## Risks

- Anthropic thinking continuity requirements are stricter than plain text replay
- tool exposure changes may affect existing prompt contents and snapshot-like request expectations
- adding `web_search` / `image_generation` parity may reveal assumptions in current tool handlers about Responses-only shapes
- stream retry behavior must avoid replaying incomplete tool loops incorrectly

## Decision

Proceed with a follow-up Anthropic parity change focused on protocol correctness first, then capability exposure and broader tool support.
