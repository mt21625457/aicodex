## Context

This change is based on a behavior-level review of the local Claude reference
implementation under `/Users/mt/code/mt-ai/cc`, especially:

- `src/utils/tokens.ts`, where `tokenCountWithEstimation()` is documented as
  the canonical threshold count;
- `src/services/compact/autoCompact.ts`, where effective context window,
  autocompact threshold, warning threshold, and blocking limit are derived;
- `src/components/PromptInput/Notifications.tsx` and
  `src/components/TokenWarning.tsx`, where footer warnings use only the most
  recent API response usage;
- `src/components/StatusLine.tsx` and `src/utils/context.ts`, where status-line
  percentages use input/cache fields only;
- `src/commands/context/context.tsx` and `src/utils/analyzeContext.ts`, where
  `/context` applies request-time transforms but can mix API totals with
  estimated category totals.

The important lesson is not a specific TypeScript implementation detail. The
portable invariant is that a user-visible "context used" value must describe the
same model-visible context that the next provider request and context-management
threshold checks use.

The target Rust implementation already exposes part of this model:

- `codex_protocol::protocol::TokenUsageInfo` has `context_tokens` and
  `context_source` fields.
- app-server v2 exposes these as `ThreadTokenUsage.contextTokens` and
  `contextSource`.
- core already records sources such as `ClaudeCountTokens`,
  `DeepseekStreamUsage`, `LocalEstimate`, `InFlightEstimate`,
  `ContextWindowFull`, and `Replay`.
- the TUI currently converts app-server token usage into a local
  `TokenUsageInfo` shape that only preserves `total`, `last`, and
  `model_context_window`; this is a likely surface where occupancy semantics can
  be lost.

## Goals / Non-Goals

**Goals:**

- Define one current-context occupancy model for Claude turns using the
  existing `context_tokens` / `context_source` contract where possible.
- Ensure threshold checks, footer warnings, status-line fields, and context
  report or debug totals share that model.
- Distinguish current context occupancy from cumulative token spend and
  per-response completion usage.
- Make transformed-history savings visible so compaction, replay, request
  normalization, or provider-side recounts do not leave stale usage in the UI.
- Preserve safe fallback behavior when native count-tokens or streamed usage is
  missing.

**Non-Goals:**

- Do not change Claude tool execution or message ordering semantics.
- Do not make native count-tokens a blocking preflight for every request.
- Do not change OpenAI Responses usage semantics as part of the Claude fix.
- Do not expose new public app-server schema fields unless existing
  `contextTokens` / `contextSource` fields cannot represent the clarified
  semantics.
- Do not copy product-specific feature gates from the reference implementation.

## Proposed Model

Use the existing context usage fields as the public contract, and optionally
introduce a small internal snapshot type owned by core context accounting:

```text
ContextUsageSnapshot {
  current_context_tokens,
  model_context_window,
  effective_context_window,
  auto_compact_threshold,
  blocking_limit,
  source, // maps to ContextTokenUsageSource
  transformed_message_count,
  estimated_new_message_tokens,
  native_count_tokens?
}
```

The exact Rust shape can differ, but the semantics should be explicit:

- `current_context_tokens` is what threshold checks and UI context usage use.
- `model_context_window` is the provider/model raw window.
- `effective_context_window` subtracts any output reservation used for context
  management decisions.
- `auto_compact_threshold` and `blocking_limit` are derived from the same
  effective window.
- `source` maps to the existing `ContextTokenUsageSource` enum and records
  whether the value came from provider usage, Claude count-tokens, DeepSeek
  stream usage, local estimation, in-flight estimation, replay, or a
  full-window sentinel.

## Accounting Order

The snapshot should be computed after the same target-runtime transforms used to
build the next Claude request. In the Rust workspace, this means auditing the
actual request path rather than copying reference-only transform names:

1. history replay and rollout reconstruction;
2. local or remote compaction results;
3. auto-compact window scoping and prefill accounting;
4. Claude request normalization that affects model-visible history;
5. native Claude count-tokens or provider-compatible fallback counting.

If native Claude count-tokens is available, it should count this transformed
request view. If not, the fallback should mirror the existing
"last reliable context plus estimated new messages" behavior and account for
known transform savings in the Rust runtime.

## Surface Semantics

### Threshold checks

Autocompact and blocking checks MUST consume the snapshot rather than
recomputing from raw messages. This prevents the threshold path from seeing one
history shape while the UI sees another.

### Footer warning

The footer warning MUST use `current_context_tokens` from the snapshot. It MUST
not use only the last assistant response usage when tool results or user
messages have been appended after that response.

### Status line

The status-line context remaining/used items SHOULD prefer `context_tokens`
when available. If they must fall back to `last_token_usage.total_tokens`, tests
MUST prove that field was populated by the same context-occupancy update rather
than by ordinary completion usage.

### App-server and TUI conversion

The app-server payload already carries `contextTokens` and `contextSource`.
Consumers that convert it into a local token model MUST preserve those fields or
derive display values before dropping them. In particular, TUI context
remaining/used calculations should not require `last.totalTokens` to be
overloaded with context occupancy when `contextTokens` is present.

### Context reports

Any context report or debug surface that presents a headline "current context
used" value MUST use the same `current_context_tokens` as the snapshot. Category
breakdowns MAY remain estimated, but when the headline comes from native
count-tokens or provider usage, the display should avoid implying exact
category totals. A debug or detail view may show the snapshot source and
estimate delta.

## Edge Cases

- Parallel tool calls: all interleaved tool results after the first split
  assistant block from the same API response count toward the next request.
- Missing or zero streamed usage: do not reset non-empty context occupancy to
  zero.
- Compaction/replay: count the post-compaction or replayed model-visible view,
  not obsolete pre-compaction history.
- Tool-result replacement or request normalization: count the content actually
  sent in the next Claude request, not raw historical tool output that has been
  removed or replaced.
- Native count-token failure: fall back locally and tag the snapshot with the
  existing `LocalEstimate` source.

## Detailed Test Plan

### Core accounting tests

- Add or extend `codex-core` Claude wire/session tests so a completed Claude
  turn with native count-tokens emits `TokenUsageInfo.context_tokens` and
  `context_source = ClaudeCountTokens`, while preserving ordinary streamed
  completion usage separately.
- Capture the outbound count-tokens request body and assert it reflects the
  adapter-visible request view: post-compaction history is used, replaced or
  normalized tool content is absent, and model-visible tool results or user
  messages appended after the last provider usage event are present.
- Cover source precedence with focused cases:
  `ProviderUsage` when ordinary provider usage is the authoritative occupancy
  value, `ClaudeCountTokens` on native success, `DeepseekStreamUsage` when
  DeepSeek-compatible stream usage is the best available value,
  `LocalEstimate` after native count failure or unsupported count-tokens,
  `InFlightEstimate` before completion, `ContextWindowFull` for full-window
  sentinel updates, and `Replay` for stored usage restoration.
- Cover missing and zero usage separately. A non-empty Claude conversation with
  missing streamed usage, zero input usage, or failed native counting must still
  emit non-zero occupancy when local estimation can produce one.
- Use full-object assertions for `TokenUsageInfo` where practical so tests
  prove `total_token_usage`, `last_token_usage`, `context_tokens`,
  `context_source`, and `model_context_window` stay internally consistent.

### Threshold tests

- Add tests where `last_token_usage.total_tokens` and `context_tokens` differ.
  Autocompact and blocking-limit decisions must follow `context_tokens`.
- Add a low-window fixture where a post-response tool result pushes the next
  request over the autocompact warning threshold. The footer warning and
  autocompact decision must agree.
- Add a compaction fixture where pre-compaction usage would trigger blocking
  but post-compaction model-visible occupancy does not. The turn must continue
  without surfacing a stale full-window error.
- Add an auto-compact scope fixture, including body-after-prefix or equivalent
  prefill behavior, to prove threshold inputs derive from the same snapshot.

### Protocol and app-server tests

- Extend `codex-app-server-protocol` conversion tests for
  `From<CoreTokenUsageInfo> for ThreadTokenUsage` to assert
  `contextTokens`, `contextSource`, `total`, `last`, and
  `modelContextWindow` together.
- Add JSON serialization coverage proving the existing camelCase fields remain
  `contextTokens` and `contextSource`, including a null-field case and a
  populated `claudeCountTokens` or `localEstimate` case.
- Add app-server notification coverage for `thread/tokenUsage/updated` so a
  replayed or live `TokenUsageInfo` reaches clients with occupancy fields
  intact and without requiring a schema change.

### TUI and display tests

- Add conversion tests for the TUI app-server token usage path so
  `contextTokens` and `contextSource` are preserved in the local TUI token
  usage model, or display values are derived before any lossy conversion.
- Add footer tests where `context_tokens` differs from
  `last.total_tokens`; the footer must show and warn from the occupancy value.
- Add status-line tests for `context-used`, `context-remaining`, and legacy
  `context-usage` items. When `context_tokens` is present, percentages and
  remaining tokens must use it; when absent, the fallback path must be explicit
  and covered.
- Add snapshot or rendering tests for a warning state, a normal status-line
  state, and a context report/debug headline. Estimated category breakdowns may
  appear, but must not contradict the canonical headline.

### Regression and negative tests

- Verify OpenAI Responses accounting is unchanged by running targeted existing
  Responses token-usage tests or adding one guard test if no targeted coverage
  exists.
- Verify cumulative spend cannot override occupancy by constructing a case
  where `total_token_usage.total_tokens` is much larger than
  `context_tokens`; context displays must use occupancy.
- Verify completion output tokens alone do not increase current context usage
  unless that output is model-visible in the next request.
- Verify source metadata remains lightweight and never includes prompts, tool
  inputs, credentials, media payloads, or provider-state payloads.

### Suggested commands

- `openspec validate align-context-usage-surfaces --strict`
- `cd codex-rs && just test -p codex-core`
- `cd codex-rs && just test -p codex-app-server-protocol`
- `cd codex-rs && just test -p codex-tui`
- `cd codex-rs && just fmt` after Rust edits
- `cd codex-rs && just fix -p codex-core` when `codex-core` changes
- `cd codex-rs && just fix -p codex-app-server-protocol` when
  `codex-app-server-protocol` changes
- `cd codex-rs && just fix -p codex-tui` when `codex-tui` changes
- Ask before running complete `cd codex-rs && just test` if the final
  implementation touches shared protocol, core, or common behavior broadly.

## Risks / Trade-offs

- Native count-tokens can add latency if run synchronously after every Claude
  turn. Keep it post-response and non-fatal; consider debouncing or fallback for
  providers that reject it repeatedly.
- A single snapshot helper can become too broad if it absorbs provider-specific
  request construction. Keep Claude-specific transforms inside Claude-owned
  request/accounting code and expose only the computed occupancy contract to
  UI/app-server consumers.
- Category breakdowns are inherently approximate when the headline uses native
  provider counting. Label or structure the UI so users do not mistake estimated
  categories for exact provider counts.
- Changing status-line semantics can affect user configuration and screenshots.
  Preserve item names (`context-used`, `context-remaining`, legacy
  `context-usage`) but correct their source to `context_tokens` when present.

## Migration Plan

1. Audit existing `TokenUsageInfo.context_tokens` production sites and document
   the intended precedence order.
2. Route autocompact/blocking checks through the same occupancy helper, or add
   tests proving the existing `get_total_token_usage()` path is equivalent.
3. Preserve `contextTokens` / `contextSource` through app-server and TUI
   conversion paths.
4. Update footer, status-line, and context report displays to prefer
   `context_tokens` for current occupancy.
5. Add fixture tests for compaction/replay-heavy histories and provider usage
   failures.
