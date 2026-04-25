# Proposal: Add Anthropic Wire API Support via `anthropic-sdk-rs`

## Summary

Add first-class support for Anthropic's Messages API as a new provider wire protocol while keeping the implementation isolated in a new workspace crate and minimizing touch points in `codex-core`.

This change will introduce:

- a new provider wire protocol value: `anthropic`
- a new workspace crate at `codex-rs/anthropic/` with crate name `codex-anthropic`
- a thin dispatch path from `codex-core` into `codex-anthropic`
- configuration support for Anthropic-backed providers using the existing provider framework

## Implementation Constraint

All newly added Anthropic support code SHOULD live under a single new folder:

- `codex-rs/anthropic/`

Exceptions are allowed only for thin integration edits that must touch existing code, such as:

- workspace wiring
- `WireApi` enum extension
- thin dispatch integration in `codex-core`
- config/schema/doc updates required to surface the new wire protocol

## Motivation

The current codebase already has a provider framework, but that framework is primarily a provider configuration layer:

- provider selection and runtime config live in `codex-rs/model-provider-info/`
- provider instances are translated into HTTP provider config in `codex-rs/codex-api/`
- turn orchestration and model request dispatch live in `codex-rs/core/`

Today, the execution path only supports a single wire protocol, `responses`. This means the existing provider framework is enough for OpenAI-compatible or Responses-compatible backends, but it is not enough for Anthropic's native Messages protocol by itself.

We want Claude support without expanding `codex-core` further or creating wide merge-conflict surfaces with upstream.

## Goals

- Reuse the existing provider framework for:
  - provider selection
  - auth/env/header/retry/timeout configuration
  - per-turn provider lookup
- Keep almost all new code in a single new folder to minimize upstream merge conflicts
- Keep Anthropic protocol logic in a new folder/crate rather than embedding it into `codex-core`
- Limit required upstream-conflict touch points to a small set of integration files
- Preserve existing session, tool runtime, rollout, and event pipelines
- Use `anthropic-sdk-rs` as the primary SDK for request execution and stream decoding

## Non-Goals

- Do not add Claude Code compatibility features such as `.claude/` config import in this change
- Do not redesign the session, rollout, or tool runtime architecture
- Do not require a generalized protocol plugin system before shipping MVP
- Do not promise full parity for every existing tool type in phase 1

## Proposed Shape

### New crate

Create a new workspace crate:

- path: `codex-rs/anthropic/`
- crate name: `codex-anthropic`

The new crate owns:

- `anthropic-sdk-rs` integration
- Anthropic request construction
- Anthropic streaming event decoding
- mapping Anthropic stream events back into existing internal `ResponseEvent` / `ResponseItem` shapes
- Anthropic-specific error mapping

### Thin `core` integration

Keep `codex-core` changes intentionally small:

- extend `WireApi` with `Anthropic`
- add one new `match` arm in `ModelClientSession::stream()`
- build a thin request view passed into `codex-anthropic`
- avoid moving Anthropic helper logic into existing `core` submodules unless strictly required

`codex-core` must remain the orchestrator, not the home of Anthropic protocol details.

### Provider framework reuse

Reuse existing `ModelProviderInfo` fields wherever possible:

- `base_url`
- `env_key`
- `experimental_bearer_token`
- `http_headers`
- `env_http_headers`
- `request_max_retries`
- `stream_max_retries`
- `stream_idle_timeout_ms`
- `wire_api`

## Phase 1 Scope

Phase 1 will support:

- text responses
- reasoning/thinking stream mapping
- tool use round trips for existing function/custom/local-shell flows
- provider selection through `model_provider`
- Anthropic provider config via `wire_api = "anthropic"`
- integration tests using `wiremock`

Phase 1 may explicitly defer:

- image input parity
- native support for all current tool kinds
- command-backed provider auth unless the target endpoint accepts bearer semantics
- advanced output-schema repair behavior

## Why This Approach

This proposal matches the current architecture:

- `core` already owns session and turn orchestration
- `tools` already own model-visible tool spec generation and runtime execution
- `rollout` and `state` already persist generic `ResponseItem` output
- the missing piece is a second protocol adapter, not a second orchestration stack

By isolating Anthropic protocol behavior in `codex-anthropic`, we get Claude support without turning `codex-core` into the permanent home of another large protocol implementation.

## Expected Impacted Areas

Small integration changes:

- `codex-rs/Cargo.toml`
- `codex-rs/model-provider-info/src/lib.rs`
- `codex-rs/core/src/client.rs`

New implementation area:

- `codex-rs/anthropic/`

Documentation updates:

- provider configuration docs
- any config schema that surfaces `wire_api`

## Risks

- Anthropic Messages semantics differ from Responses semantics in tool and reasoning streaming
- current provider auth abstractions are bearer-oriented, while Anthropic commonly uses `x-api-key`
- `Prompt` is currently defined in `codex-core`, so the integration boundary must stay thin and carefully chosen

## Decision

Proceed with a dedicated `codex-anthropic` crate that reuses the provider framework and keeps `codex-core` changes thin and local.
