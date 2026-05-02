## Context

Claude Messages streaming currently maps provider usage from `message_start` and
`message_delta` into Codex `TokenUsage`. That usage is useful for the response
that just completed, but it is not always a reliable measure of the full current
context. Anthropic-compatible gateways may omit usage, return only output usage,
or return zeros. Codex then emits a `thread/tokenUsage/updated` notification
with zero or partial values, and AICodex renders the context window as empty.

The Claude endpoint client already supports `POST /v1/messages/count_tokens`.
That endpoint is the protocol-native way to ask Claude how many tokens the
request context would consume. The missing piece is wiring this endpoint into
Claude turn orchestration after history has been updated, while preserving the
existing OpenAI Responses path and Claude streaming/tool-loop behavior.

## Goals / Non-Goals

**Goals:**

- Refresh Claude context-window usage with native count-token results after a
  successful turn when the active provider uses `WireApi::Claude`.
- Treat missing, zero, or incomplete streamed Claude usage as unsuitable for
  context occupancy while still preserving useful per-response counters where
  available.
- Fall back to local estimation when count-token calls fail or are unsupported.
- Emit the existing `TokenCount` / `thread/tokenUsage/updated` contract without
  schema changes.
- Keep Claude auth, endpoint paths, and request serialization inside Claude
  adapter boundaries.

**Non-Goals:**

- Do not call count-tokens for OpenAI Responses providers.
- Do not make count-tokens a preflight dependency before every Claude request.
- Do not change Claude tool execution, pause-turn continuation, prompt caching,
  or content-block preservation semantics.
- Do not expose a new app-server notification or frontend-only workaround.

## Decisions

### 1. Count after history is updated

The count-token request should be built after the assistant response and any
tool results required for the just-finished turn are represented in history.
That makes the count match what the next Claude request would send and therefore
what the UI should label as current context usage.

Alternative considered: count before sending the model request. That would help
admission checks, but it would not include the assistant answer that just
expanded the live context and would not fix the post-answer UI reset.

### 2. Scope the behavior to `WireApi::Claude`

The turn loop should branch at the provider boundary and call a Claude-owned
counting helper only when the resolved provider uses `WireApi::Claude`. OpenAI
Responses providers continue to use completion usage from the Responses SSE
parser.

Alternative considered: add generic count-token behavior to all providers. That
would create a larger provider API surface and risk changing established
Responses accounting for no benefit to this bug.

### 3. Preserve completion usage, but prefer count usage for context occupancy

Claude streamed usage still carries useful ordinary input, output, and cache
read/write counters. The final context-usage notification should use native
count-token `input_tokens` as `last_token_usage.total_tokens` for the live
context estimate. When streamed usage is present, it may still be appended to
`total_token_usage` for cumulative response counters, but zero/partial streamed
usage must not be allowed to overwrite the current context occupancy.

Alternative considered: ignore streamed Claude usage entirely. That would solve
the zero reset but would drop cache accounting already covered by Claude SSE
tests.

### 4. Use local estimation as a resilient fallback

If `/messages/count_tokens` fails due to unsupported endpoint, network errors,
rate limits, malformed responses, or provider compatibility issues, Codex should
fall back to `ContextManager::estimate_token_count_with_base_instructions`.
The fallback emits a non-zero context estimate when the conversation is non-empty
and preserves the existing model context window metadata.

Alternative considered: surface the count-token error to the user. That would be
too noisy for a UI accounting refresh and would make Claude-compatible gateways
that lack the endpoint look broken even when inference itself works.

### 5. Keep notification shape stable

The app-server `thread/tokenUsage/updated` payload remains
`{ total, last, modelContextWindow }`. This avoids frontend IPC or daemon
contract churn. Tests should assert payload values rather than introducing a new
method.

## Risks / Trade-offs

- Count-token latency after every Claude turn can add a small post-response
  delay before the usage badge refreshes -> Run it after streaming completion so
  answer rendering is not blocked; keep fallback local and bounded.
- Some gateways may reject `/messages/count_tokens` -> Treat failure as
  non-fatal and use local estimation.
- Native count and streamed billing usage measure different things -> Document
  the distinction in tests and keep the UI-facing context occupancy derived from
  the count result.
- Tool-loop follow-up turns may update history multiple times -> Count after the
  turn loop installs model-visible history so the result reflects the next
  request context.

## Migration Plan

1. Add Claude context counting helpers in `codex-core` near turn/session token
   accounting.
2. Reuse `ClaudeCountTokensRequest::from(&ClaudeMessagesApiRequest)` and the
   existing Claude endpoint client.
3. Update Claude completion handling to refresh context usage with native count
   or fallback estimation.
4. Add unit and mocked integration coverage.
5. Roll back by disabling the Claude-only count refresh; existing streamed usage
   and local estimation code paths remain available.
