# Tasks

## 1. Protocol correctness

- [x] Pass Anthropic reasoning controls through the thin DTO boundary
- [x] Build Anthropic thinking request controls from existing per-turn effort/summary
- [x] Preserve raw thinking blocks and signatures in Anthropic stream handling
- [x] Replay raw thinking blocks when Anthropic history is rebuilt after tool use
- [x] Support structured and image-bearing Anthropic `tool_result` content
- [x] Support Anthropic image input blocks for user messages

## 2. Tool-use safety

- [x] Require `content_block_stop` before finalizing Anthropic tool-use input
- [x] Reject incomplete or invalid `input_json_delta` payloads instead of executing partial tool calls
- [x] Reject missing Anthropic tool-use ids/names as protocol errors
- [x] Add regression coverage for truncated tool-use blocks

## 3. Stop reasons and stream errors

- [x] Handle Anthropic stop reasons explicitly instead of treating all stops as successful completion
- [x] Map Anthropic SSE `event:error` payloads such as `overloaded_error`
- [x] Improve Anthropic cache token accounting
- [ ] Evaluate and document safe stream retry behavior for Anthropic transport failures

## 4. Tool capability exposure

- [x] Add provider-aware tool filtering before Anthropic request construction
- [x] Keep adapter-level explicit errors for unsupported historical Anthropic items
- [x] Add Anthropic-facing schema + round-trip support for `web_search`
- [x] Evaluate whether `tool_search` should be exposed or filtered for Anthropic
- [x] Evaluate Anthropic parity plan for `image_generation`

## 5. Per-turn controls

- [x] Map `service_tier` onto Anthropic request semantics
- [x] Decide whether `turn_metadata_header` remains internal-only or is forwarded as a custom per-request header

## 6. Tests

- [x] Add unit tests in `codex-anthropic` for:
- [x] thinking request controls
- [x] thinking replay
- [x] structured/image tool results
- [x] image inputs
- [x] overloaded SSE errors
- [x] stop-reason failures
- [x] cache token accounting
- [x] Add core Anthropic integration tests for:
- [x] provider-aware tool exposure
- [x] `service_tier` request serialization
- [x] any newly supported Anthropic tool types

## 7. Docs

- [x] Update `docs/config.md` Anthropic capability table
- [x] Update `doc/anthropic-manual-test-config.md`
- [x] Document any remaining unsupported Anthropic tools explicitly
