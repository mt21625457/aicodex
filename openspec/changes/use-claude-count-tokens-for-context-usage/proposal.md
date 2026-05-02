## Why

Claude Messages usage events are not a reliable source of current context-window
occupancy for Codex UI surfaces. Some Claude-compatible providers omit usage or
emit zero/partial usage after a streamed answer, which makes downstream
`thread/tokenUsage/updated` notifications show the context as empty even though
the conversation history remains live.

## What Changes

- Use Anthropic's native `POST /v1/messages/count_tokens` capability to refresh
  Claude context usage after a successful streaming turn.
- Keep normal streaming completion usage for billing-style per-response
  counters, but do not let missing or zero Claude usage overwrite current
  context occupancy.
- Add a safe fallback to the existing local context estimator when Claude token
  counting is unavailable, rejected, rate-limited, or not applicable.
- Preserve the existing OpenAI Responses accounting path and keep Claude-specific
  behavior behind the `WireApi::Claude` protocol boundary.
- Add mocked Claude end-to-end tests covering missing streamed usage, successful
  native count refresh, and count-token fallback behavior.

## Capabilities

### New Capabilities

- `claude-context-usage-accounting`: Claude providers report current context
  window usage from native count-tokens results when available, with local
  estimation as a fallback.

### Modified Capabilities

- `claude-wire-api-support`: Claude Messages routing gains a post-turn context
  accounting step that uses the Claude endpoint adapter without changing request
  roles, tool-loop semantics, streaming event handling, or Anthropic
  authentication boundaries.

## Impact

- Affected crates:
  - `codex-rs/core`
  - `codex-rs/codex-api`
- Affected protocol surface:
  - Existing `thread/tokenUsage/updated` notifications keep the same shape, but
    their `last.totalTokens` value becomes current context occupancy for Claude
    turns instead of unreliable streamed usage when native counting succeeds or
    fallback estimation is needed.
- Compatibility:
  - OpenAI Responses behavior is unchanged.
  - Claude-compatible providers that do not implement `/messages/count_tokens`
    degrade to local estimation instead of showing zero usage.
  - Claude authentication headers remain scoped to Claude endpoint adapters.
- Verification:
  - Targeted `cargo test` for Claude SSE/client tests.
  - Targeted `cargo test` for core/app-server scenarios that emit
    `thread/tokenUsage/updated`.
  - `cargo check` for the Rust workspace touched by the change.
