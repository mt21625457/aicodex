## 1. OpenSpec and Scope

- [x] 1.1 Keep this proposal, design, and spec delta synchronized with the implementation.
- [x] 1.2 Confirm the change stays in the protocol adapter boundary and does not add Claude logic to the OpenAI Responses parser.

## 2. Wire API Adapter Abstraction

- [x] 2.1 Define a reusable adapter boundary for provider endpoint/header policy, request conversion, tool serialization, stream accumulation, and conformance tests.
- [x] 2.2 Ensure core dispatch selects by `WireApi` and delegates protocol-specific behavior to adapter modules instead of branching through the shared turn loop.
- [x] 2.3 Add or document adapter conformance test helpers that future protocols can reuse for path/header, text stream, tool loop, usage, and error scenarios.

## 3. Provider Routing and Auth

- [x] 3.1 Add `WireApi::Claude` provider selection and default Anthropic base URL behavior.
- [x] 3.2 Add Anthropic auth handling that sends `x-api-key` and does not send OpenAI bearer credentials for Claude providers.
- [x] 3.3 Document Claude provider configuration in `docs/config.md`.
- [x] 3.4 If provider config schema changes, regenerate `codex-rs/core/config.schema.json`.

## 4. Claude Request and History Adapter

- [x] 4.1 Add typed Claude Messages request/content structs for text, base64 image, `tool_use`, `tool_result`, thinking, usage, and stream events.
- [x] 4.2 Convert Codex base/system/developer instructions to Claude top-level `system`.
- [x] 4.3 Convert Codex user/assistant history to valid Claude `messages`, preserving tool-use/tool-result ordering.
- [x] 4.4 Support base64 image data URLs and HTTP(S) image URLs as Claude image blocks, while explicitly handling unsupported image inputs.
  - Added adapter coverage for unsupported image references degrading to explicit text placeholders.
- [x] 4.5 Add unit tests comparing complete serialized Claude request objects, including tool history.
  - Added direct Claude wire-shape tests for text, base64 image, URL image, `tool_use`, text/error/structured `tool_result`, thinking signatures, `tool_choice`, `thinking`, `service_tier`, and request side-table omission.
- [x] 4.6 Map Codex reasoning effort and service tier settings to Claude `thinking` and `service_tier` request fields.
  - Added coverage for both `fast -> auto` and `flex -> standard_only`.
  - Added wire coverage for Anthropic `thinking` disabled variant while the Codex adapter continues to emit enabled/absent based on current behavior.
- [x] 4.7 Preserve structured multimodal tool outputs as Claude `tool_result` content blocks.
  - Added coverage for structured `tool_result` blocks that contain text, base64 image, and URL image content.

## 5. Tool Serialization and Reverse Mapping

- [x] 5.1 Serialize Codex function tools to Claude `tools` with `name`, `description`, and `input_schema`.
- [x] 5.2 Flatten namespaced MCP tools into Claude-valid names and preserve reverse mapping to Codex namespace/name.
- [x] 5.3 Represent custom/freeform tools through a Claude-compatible schema while reconstructing `CustomToolCall` on stream output.
- [x] 5.4 Represent local shell and tool search in a way that preserves existing Codex execution semantics.
- [x] 5.5 Add tests for invalid characters, long names, collisions, custom tools, local shell, and tool search.

## 6. Claude SSE Parser

- [x] 6.1 Parse `message_start`, `content_block_start`, `content_block_delta`, `content_block_stop`, `message_delta`, `message_stop`, and `error`.
- [x] 6.2 Accumulate `text_delta` into output text deltas and final message items.
- [x] 6.3 Accumulate `input_json_delta` fragments per block and parse the final JSON at `content_block_stop`.
- [x] 6.4 Accumulate `thinking_delta` and `signature_delta` without mixing thinking content into ordinary output text.
- [x] 6.5 Merge usage from start/delta events, including cache creation/read tokens where present.
  - Added stream accumulator coverage for multiple text/tool JSON fragments, stop metadata parsing, and cache token accounting.
- [x] 6.6 Surface malformed events and stream errors as actionable `ApiError`s.
  - Added Claude HTTP JSON error mapping so Anthropic error envelopes surface as `ApiError::Api`/rate-limit errors instead of only raw transport bodies.
  - Added Anthropic `overloaded_error` mapping to Codex server-overloaded errors.
  - Added coverage for Claude SSE `error` events and `content_block_start` before `message_start`.
- [x] 6.7 Reject out-of-order or mismatched Claude stream deltas before they can create invalid Codex items.

## 7. Mock End-to-End Coverage

- [x] 7.1 Add a mock transport/server test proving Claude requests hit `/v1/messages`.
- [x] 7.2 Assert request headers include `x-api-key`, `anthropic-version: 2023-06-01`, and `Accept: text/event-stream`.
- [x] 7.3 Run a mocked tool-use turn where Claude streams `tool_use`, Codex executes a client-side tool, and the next request sends `tool_result`.
  - Added coverage for multiple `tool_use` blocks in one assistant turn and verified the following user `tool_result` blocks preserve order.
  - Added coverage for client-side tool execution errors returning Claude `tool_result` blocks with `is_error: true`.
- [x] 7.4 Cover text, base64 image, URL image, structured tool-result content, function call, custom/freeform, local shell, and tool-search request/response shapes.
- [x] 7.5 Cover thinking/signature, usage, and error stream scenarios.
- [x] 7.6 Add a future-protocol adapter conformance test fixture or checklist so the next wire API does not start from scratch.

## 8. Verification

- [x] 8.1 Run `cd codex-rs && just fmt`.
- [x] 8.2 Run targeted tests for changed crates: `codex-tools`, `codex-api`, `codex-model-provider-info`, `codex-model-provider`, `codex-config`, and `codex-core`.
- [x] 8.3 Run scoped `just fix -p <crate>` for changed Rust crates after tests pass.
  - Initial runs were blocked locally because `rustup` marked clippy installed for `1.93.0-aarch64-apple-darwin`, but the toolchain `bin/` directory was missing `cargo-clippy` and `clippy-driver`.
  - Removed and reinstalled the clippy component for `1.93.0-aarch64-apple-darwin`; `cargo clippy -V` now reports `clippy 0.1.93`.
  - `just fix -p codex-api` passed and applied one automatic fix in `codex-api/src/common.rs`.
  - `just fix -p codex-core` passed after applying one automatic fix in `core/src/claude.rs` and manually resolving the remaining Claude-related `unnecessary_filter_map` and `expect_used` lints.
- [x] 8.4 Record any tests intentionally deferred because they require the full Rust suite or external services.
  - No external-service tests were run. Full `cargo test -p codex-core` was attempted and hit three unrelated agent/multi-agent timeout failures; the Claude-specific `codex-core` coverage passes.
