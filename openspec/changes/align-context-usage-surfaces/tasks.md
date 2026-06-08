## 1. OpenSpec and Scope

- [x] 1.1 Validate this change against existing Claude context usage proposals
  and keep it scoped to accounting/display consistency.
- [x] 1.2 Confirm existing `ThreadTokenUsage.contextTokens` and
  `contextSource` fields are sufficient; avoid app-server schema changes unless
  tests prove they cannot represent the corrected semantics.
- [x] 1.3 Run `openspec validate align-context-usage-surfaces --strict`.

## 2. Canonical Context Usage Snapshot

- [x] 2.1 Audit existing `TokenUsageInfo.context_tokens` and
  `ContextTokenUsageSource` production sites in core.
- [x] 2.2 Add or formalize an internal context occupancy helper if existing
  helpers cannot express current context tokens, model window, threshold inputs,
  and source consistently.
- [x] 2.3 Build Claude occupancy from the transformed model-visible request view
  after the Rust runtime's actual compaction, replay, request normalization, and
  provider counting steps.
- [x] 2.4 Reuse the existing source precedence where possible:
  `ProviderUsage`, `ClaudeCountTokens`, `DeepseekStreamUsage`,
  `LocalEstimate`, `InFlightEstimate`, `ContextWindowFull`, and `Replay`.
- [x] 2.5 Add unit tests for missing usage, zero usage, native count failure,
  and local fallback source metadata.
- [x] 2.6 Ensure Claude count-token or fallback accounting consumes the
  adapter-owned model-visible request view rather than reconstructing
  transform semantics in UI or app-server code.

## 3. Threshold Consistency

- [x] 3.1 Route autocompact checks through the snapshot.
- [x] 3.2 Route blocking-limit checks through the snapshot.
- [x] 3.3 Add tests proving threshold decisions use transformed history rather
  than stale pre-compaction, pre-replay, or completion-only usage.

## 4. User-Visible Context Surfaces

- [x] 4.1 Preserve `contextTokens` and `contextSource` when app-server
  `ThreadTokenUsage` is converted into TUI token usage models.
- [x] 4.2 Update footer/TUI context warnings to prefer snapshot
  `context_tokens` over `last.total_tokens` when available.
- [x] 4.3 Update status-line context-window fields to report snapshot occupancy
  or clearly separate occupancy from cumulative token spend.
- [x] 4.4 Update context report/debug surfaces to match the snapshot headline
  while keeping category breakdowns labeled as estimated when appropriate.
- [x] 4.5 Add snapshot or rendering coverage for warning, status-line, and
  context-report consistency.

## 5. Transform and Tool History Coverage

- [x] 5.1 Add tests for tool results or user items appended after the last
  provider usage event.
- [x] 5.2 Add tests for context usage after local/remote compaction and replay.
- [x] 5.3 Add tests for request normalization or replacement that removes
  model-visible content before provider counting.
- [x] 5.4 Add tests that estimated category or cumulative totals do not override
  the canonical occupancy headline.

## 6. Detailed Test Coverage

- [x] 6.1 Add `codex-core` tests for successful native Claude count-tokens
  refresh, asserting `context_tokens`, `context_source`, `last_token_usage`,
  `total_token_usage`, and `model_context_window` together.
- [x] 6.2 Add `codex-core` tests that capture the count-tokens request body and
  prove accounting uses the adapter-visible request view after compaction,
  replay, request normalization, and tool-result replacement.
- [x] 6.3 Add `codex-core` tests for source precedence:
  `ProviderUsage`, `ClaudeCountTokens`, `DeepseekStreamUsage`,
  `LocalEstimate`, `InFlightEstimate`, `ContextWindowFull`, and `Replay`.
- [x] 6.4 Add separate missing-usage and zero-usage tests proving a non-empty
  Claude conversation does not display empty occupancy when native counting or
  local estimation can provide a count.
- [x] 6.5 Add threshold tests where `context_tokens` differs from
  `last_token_usage.total_tokens`, proving autocompact and blocking-limit
  decisions use the canonical snapshot.
- [x] 6.6 Add a post-response tool-result or user-message threshold test where
  footer warning state and autocompact state agree.
- [x] 6.7 Add a compaction/replay test where stale pre-compaction usage would
  exceed a threshold but post-transform occupancy does not.
- [x] 6.8 Extend `codex-app-server-protocol` conversion and JSON serialization
  tests for populated and null `contextTokens` / `contextSource` values.
- [x] 6.9 Add app-server notification coverage proving
  `thread/tokenUsage/updated` preserves occupancy fields for live and replayed
  updates.
- [x] 6.10 Add TUI conversion tests proving app-server `contextTokens` and
  `contextSource` survive into the local token model or are used before any
  lossy conversion.
- [x] 6.11 Add TUI footer tests where `context_tokens` differs from
  `last.total_tokens`; warning copy and thresholds must use occupancy.
- [x] 6.12 Add TUI status-line tests for `context-used`,
  `context-remaining`, and legacy `context-usage` items with both populated
  and absent `context_tokens`.
- [x] 6.13 Add snapshot or rendering coverage for a warning state, a normal
  status-line state, and a context report/debug headline with estimated
  categories clearly secondary.
- [x] 6.14 Add regression tests proving cumulative spend, completion output
  tokens, and estimated category totals cannot override canonical occupancy.
- [x] 6.15 Add a guard test or run targeted existing coverage proving OpenAI
  Responses accounting is unchanged.
- [x] 6.16 Add assertions that context source metadata remains lightweight and
  does not include prompts, tool inputs, credentials, media payloads, or
  provider-state payloads.

## 7. Verification Commands

- [x] 7.1 Run `openspec validate align-context-usage-surfaces --strict`.
- [x] 7.2 Run `cd codex-rs && just fmt` after Rust edits.
- [x] 7.3 Run targeted `cd codex-rs && just test -p codex-core ...` coverage
  for core accounting, transform, compaction, and threshold changes.
- [x] 7.4 Run targeted `cd codex-rs && just test -p codex-app-server-protocol
  ...` coverage for protocol conversion or schema-shape changes.
- [x] 7.5 Run targeted `cd codex-rs && just test -p codex-tui ...` coverage
  for TUI conversion, footer, status-line, or snapshot changes.
- [x] 7.6 Run targeted `cd codex-rs && just test -p codex-app-server ...`
  coverage for live and replayed token-usage notifications.
- [x] 7.7 Run crate-scoped `just fix -p <project>` for each Rust crate changed
  before finalizing implementation.
- [x] 7.8 Ask before running the complete Rust workspace test suite if common,
  core, or protocol changes require broader validation.
