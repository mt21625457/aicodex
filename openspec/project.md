# Project Context

## Purpose

This repository contains the Rust implementation of Codex. It coordinates model-provider calls, tool execution, TUI/app surfaces, configuration, and protocol types used by the Codex client runtime.

## Tech Stack

- Rust workspace under `codex-rs/`.
- Provider HTTP/SSE transport lives in `codex-rs/codex-api`.
- Provider metadata and auth selection live in `codex-rs/model-provider-info` and `codex-rs/model-provider`.
- Tool wire serialization lives in `codex-rs/tools`.
- Turn orchestration and provider dispatch live in `codex-rs/core`.

## Project Conventions

- Prefer protocol adapters over mixing provider-specific logic into shared orchestration paths.
- Keep new Rust modules small and focused; avoid growing central `codex-core` files when an adapter crate can own the behavior.
- Run `just fmt` from `codex-rs` after Rust edits.
- Run targeted crate tests for changed crates, and ask before running the full Rust suite when common/core/protocol changes require it.
- Update config schema/docs when `ConfigToml` or provider configuration changes.

## Domain Context

Codex internally uses `ResponseItem` and `ResponseEvent` to represent messages, reasoning, tool calls, and tool outputs. Provider-specific wire APIs must translate to and from that internal model without changing the client-side tool loop semantics.

## Important Constraints

- Claude Messages support must use `/v1/messages` semantics and must not route through the OpenAI Responses request/stream parser.
- Claude wire request and stream parsing should be typed at the adapter boundary; arbitrary JSON should be limited to tool input schemas and model-provided tool input payloads.
- Existing client-side function, custom/freeform, local shell, and tool search loops must continue to operate through `ResponseItem`.
