## Why

The local Claude reference implementation inspected for this change shows that
context-window accounting can drift when different surfaces use different token
sources. The production symptoms are subtle: an autocompact decision can be made
from one estimate, the footer warning can display another value, the status line
can report a third percentage, and the reference `/context` command can show a
category breakdown that does not match the headline total.

The key divergence observed in the reference implementation is:

- admission and autocompact checks use a "last API usage plus estimated new
  messages" context count;
- footer warnings use only the most recent API response usage and can miss tool
  results or user content appended after that response;
- status-line percentages use input/cache usage only and do not include
  output tokens or post-response additions;
- the reference `/context` command applies request-time transforms, but its
  displayed total can be replaced by stale API usage while the category
  breakdown remains estimated.

Codex already has protocol room for this distinction through
`ThreadTokenUsage.contextTokens` and `contextSource`. The missing optimization
is to make those existing occupancy fields the shared source of truth for
threshold checks and UI/status surfaces, rather than allowing consumers to fall
back to `last` or `total` token usage with different semantics.

## What Changes

- Standardize the existing `TokenUsageInfo.context_tokens` /
  `ThreadTokenUsage.contextTokens` value as the canonical context-occupancy
  snapshot for Claude providers.
- Ensure the snapshot represents the model-visible request context after the
  target Rust runtime's request-time transforms, such as compaction, replay,
  request normalization, and provider-specific context counting. Reference-only
  transforms observed in `cc` (for example snip, microcompact, or context
  collapse) are examples of the class of bug, not new Codex features required by
  this change.
- Use the same snapshot for:
  - autocompact and blocking threshold checks;
  - TUI/footer context warnings;
  - status-line `context_window` fields;
  - any context report or debug surface that presents "current context used".
- Keep cumulative billing/completion usage separate from current context
  occupancy so output tokens, cache tokens, and native count-token results are
  not mixed ambiguously.
- Preserve and consume existing source metadata such as `providerUsage`,
  `claudeCountTokens`, `deepseekStreamUsage`, `localEstimate`,
  `inFlightEstimate`, and `replay`.
- Add tests for post-response tool results, compaction/replay, missing streamed
  usage, app-server token usage conversion, footer/status-line consistency, and
  stale fallback behavior.

## Capabilities

### Modified Capabilities

- `claude-context-usage-accounting`: Claude context usage becomes a single
  canonical occupancy snapshot shared by threshold checks and user-visible
  context displays.
- `claude-wire-api-support`: Claude request-time transforms expose enough
  accounting information for context usage to match the next provider request
  without changing tool-loop semantics.

## Impact

- Affected crates:
  - `codex-rs/core` for turn-level context snapshot creation and threshold
    decisions.
  - `codex-rs/codex-api` only if native Claude count-token responses need
    additional metadata surfaced to core.
  - `codex-rs/app-server-protocol` only if existing fields prove insufficient;
    the expected path is to preserve the current payload shape.
  - `codex-rs/app-server` and `codex-rs/tui` surfaces that convert or render
    `ThreadTokenUsage`.
- Affected protocol surface:
  - Existing `thread/tokenUsage/updated` notifications SHOULD keep their shape.
    `contextTokens` and `contextSource` MUST represent current context
    occupancy; `total` and `last` MUST remain cumulative/session and
    per-response or context-update breakdowns as documented by the core
    `TokenUsageInfo` contract.
- Compatibility:
  - OpenAI Responses accounting is unchanged unless a shared provider-neutral
    snapshot helper can be adopted without changing behavior.
  - Claude-compatible providers that omit usage continue to degrade to local
    estimation instead of resetting context usage.
  - Tool execution, prompt caching, and request serialization semantics are
    unchanged.
- Verification:
  - OpenSpec validation for this change.
  - Targeted `codex-core` tests for Claude context accounting, request
    transforms, threshold decisions, and fallback source precedence.
  - Targeted `codex-app-server-protocol` and app-server notification tests
    proving `contextTokens` and `contextSource` survive serialization and
    conversion.
  - Targeted `codex-tui` tests or snapshots for footer warnings, status-line
    context displays, and context report/debug surfaces.
  - Regression tests proving cumulative token spend, completion usage, and
    estimated category totals cannot override the canonical occupancy headline.
