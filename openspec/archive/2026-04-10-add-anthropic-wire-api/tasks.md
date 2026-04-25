# Tasks

## 1. Provider and workspace wiring

- [x] Add workspace member `codex-rs/anthropic`
- [x] Add `anthropic-sdk-rs` as a workspace dependency
- [x] Extend `codex-model-provider-info::WireApi` with `Anthropic`
- [x] Update `WireApi` parsing, display, and tests

## 2. New Anthropic adapter crate

- [x] Create `codex-rs/anthropic/` with crate name `codex-anthropic`
- [x] Keep all newly added Anthropic implementation files inside `codex-rs/anthropic/` unless an exception is documented
- [x] Create initial file layout:
- [x] `src/lib.rs`
- [x] `src/dto.rs`
- [x] `src/auth.rs`
- [x] `src/request.rs`
- [x] `src/tool_mapping.rs`
- [x] `src/stream.rs`
- [x] `src/error.rs`
- [x] Add request DTOs for the thin integration boundary from `core`
- [x] Implement provider credential/header resolution using existing provider config fields
- [x] Implement Anthropic request construction from the thin DTOs
- [x] Implement stream event conversion into internal `ResponseEvent`
- [x] Implement error mapping into `CodexErr`

## 3. Thin core integration

- [x] Add a new `WireApi::Anthropic` dispatch arm in `codex-core` model streaming
- [x] Keep `core` changes limited to request conversion and dispatch
- [x] Avoid moving tool runtime, rollout, or session logic into the new crate
- [x] Document any unavoidable edits outside `codex-rs/anthropic/` as explicit exceptions

## 4. Tool compatibility

- [x] Support `Function` tool specs
- [x] Support `Freeform` tool specs
- [x] Support `LocalShell` tool specs
- [x] Defer unsupported tool types explicitly and document them

## 5. Tests

- [x] Add unit tests in `codex-anthropic` for request mapping
- [x] Add unit tests in `codex-anthropic` for stream event mapping
- [x] Add core integration tests for:
- [x] text streaming
- [x] reasoning deltas
- [x] tool use round trip
- [x] auth/header behavior
- [x] error mapping

## 6. Docs

- [x] Document Anthropic provider config example
- [x] Update config schema and related docs if `wire_api` surface changes
- [x] Document phase-1 limitations
