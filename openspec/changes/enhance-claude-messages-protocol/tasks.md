## 1. OpenSpec and Scope

- [x] 1.1 Audit the proposal against implementation reality and remove over-broad public config, provider-neutral token-estimate, and beta-header commitments from this change.
- [x] 1.2 Keep Claude enhancements inside Claude adapter modules or narrow provider-neutral history shapes already present in the repo.
- [x] 1.3 Keep Claude Code CLI harness integration out of this change; capture it as a future proposal if needed.

## 2. Prompt Caching

- [x] 2.1 Add typed Claude cache-control structs/enums for supported request locations.
- [x] 2.2 Update Claude request serialization so system prompts can be emitted as text blocks when cache metadata is needed.
- [x] 2.3 Add a conservative cache-placement policy for stable tools/system prefixes.
- [x] 2.4 Add optional conversation-prefix cache placement for long histories where the prior turn is stable enough to reuse.
- [x] 2.5 Keep public config/schema unchanged in this proposal; use internal Claude request policy options for now.
- [x] 2.6 Add request-shape tests for cache disabled, system-prefix cache, conversation-prefix cache, and current-turn no-cache behavior.
- [x] 2.7 Add mocked Claude SSE usage tests proving cache creation/read tokens continue to be reflected in Codex `TokenUsage`.

## 3. Claude Token Counting

- [x] 3.1 Add a `POST /v1/messages/count_tokens` client in `codex-api` using Claude endpoint/auth/header policy.
- [x] 3.2 Reuse Claude prompt/history/tool serialization for count-token requests while omitting fields that are invalid for the count endpoint.
- [x] 3.3 Keep token counting at the Claude endpoint-client boundary in this change; defer provider-neutral estimate integration.
- [x] 3.4 Prove ordinary Claude streaming turns do not call the count-token endpoint.
- [x] 3.5 Add mock HTTP tests for count-token path, headers, request body, success response, and Anthropic error envelope mapping.

## 4. Content Block Forward Compatibility

- [x] 4.1 Inventory current Claude stream/request block handling and identify unsupported Anthropic block types that need typed support.
- [x] 4.2 Add typed Claude content block variants for protocol-relevant blocks that Codex can project or round-trip.
- [x] 4.3 Reuse `ResponseItem::Compaction` as the smallest provider-neutral representation for Claude provider-state blocks.
- [x] 4.4 Ensure unsupported user-visible blocks degrade to explicit placeholders or diagnostics rather than disappearing silently.
- [x] 4.5 Add stream accumulator tests for known additional block behavior, unknown user-visible blocks, and provider-state blocks.
- [x] 4.6 Add history serialization tests proving preserved provider-state blocks are emitted back to Claude correctly.

## 5. Pause-Turn and Continuation Handling

- [x] 5.1 Preserve Claude `stop_reason` internally so the turn loop can distinguish `tool_use` from `pause_turn`.
- [x] 5.2 Add continuation logic for `pause_turn` that includes the paused assistant content in the next Claude request.
- [x] 5.3 Add a bounded retry/continuation cap and surface an actionable error if Claude repeatedly pauses without completing.
- [x] 5.4 Add mocked end-to-end tests for a pause-turn continuation that eventually completes.
- [x] 5.5 Add mocked end-to-end tests for the continuation cap.

## 6. Compaction and Provider State

- [x] 6.1 Decide that current `ResponseItem::Compaction` can carry Claude provider-state JSON.
- [x] 6.2 Add request and stream support for Claude compaction/provider-state blocks returned by the provider.
- [x] 6.3 Keep public Anthropic beta-header configuration out of this change; future beta headers must stay endpoint-scoped.
- [x] 6.4 Add round-trip tests proving compaction/provider-state blocks survive across turns.
- [x] 6.5 Update docs to explain supported compaction behavior and current config limitations.

## 7. Documentation and Manual Testing

- [x] 7.1 Update `docs/config.md` with Claude prompt caching, token counting, continuation, and feature limitation notes.
- [x] 7.2 Update `test/claude-wire-api/README.md` with manual cache/count/continuation smoke-test guidance.
- [x] 7.3 Document that no public cache mode, TTL, or Claude beta-header config is added by this proposal.

## 8. Verification

- [x] 8.1 Run `cd codex-rs && just fmt`.
- [x] 8.2 Run targeted tests for changed crates: `codex-api` and `codex-core`.
- [x] 8.3 Run scoped `just fix -p <crate>` for changed Rust crates after tests pass.
- [x] 8.4 Run `openspec validate enhance-claude-messages-protocol --strict`.
- [x] 8.5 Record unrelated full `codex-core` baseline failures: config schema fixture drift and two agent/control sqlite persistence timeouts.
