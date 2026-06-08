## Context

The Claude Messages adapter already has typed stream parsing and the shared
API bridge already maps `ApiError::ContextWindowExceeded` to
`CodexErr::ContextWindowExceeded`. The missing link is behavioral: Claude's
`model_context_window_exceeded` stop reason is still allowed to finish as a
normal stream completion, and the core turn loop reports
`CodexErr::ContextWindowExceeded` as an ordinary terminal error instead of
first attempting the same auto-compaction recovery used for other context-limit
paths.

The relevant ownership stays unchanged:

- `codex-rs/codex-api/src/sse/claude.rs` owns Claude stream parsing and must
  classify the stop reason as an API error.
- `codex-rs/codex-api/src/api_bridge.rs` owns provider-error to core-error
  mapping and should continue using the existing context-window variant.
- `codex-rs/core/src/session/turn.rs` owns turn orchestration, auto-compaction,
  retries, pending-input draining, and terminal client error emission.
- app-server/frontend remain consumers of existing error and token-usage
  surfaces.

## Goals / Non-Goals

**Goals:**

- Make Claude context-window overflow enter the same provider-neutral recovery
  chain as OpenAI Responses.
- Attempt exactly one mid-turn auto-compaction and retry before surfacing a
  terminal context-window error.
- Keep pending steer from being drained ahead of the retry that resumes the
  current model/tool continuation.
- Add a conservative pre-sampling estimate that includes incoming turn material
  before deciding whether to compact.
- Prove success and failure behavior with Claude SSE and core turn-loop tests.

**Non-Goals:**

- Do not add app-server or frontend protocol events.
- Do not change `thread/tokenUsage/updated` payload shape.
- Do not add mandatory Claude `/messages/count_tokens` preflight calls before
  every request.
- Do not change OpenAI Responses stream parsing or recovery semantics.
- Do not introduce broad provider-specific context-window logic into shared
  code beyond existing provider-neutral error variants.

## Decisions

### 1. Claude SSE emits an error for hard context overflow

When `ClaudeStreamEvent::MessageDelta` carries
`ClaudeStopReason::ModelContextWindowExceeded`, the Claude SSE accumulator must
return `ApiError::ContextWindowExceeded`. It may do this immediately while
handling the message delta or at `finish()` before any completed event is
emitted. The invariant is that this stream never yields a normal
`ResponseEvent::Completed` for that stop reason.

Alternative considered: preserve a completed event with
`provider_stop_reason = "model_context_window_exceeded"` and let core inspect
provider metadata. That keeps Claude-specific stop reasons in the turn loop and
does not reliably use the existing `ContextWindowExceeded` chain, so it is not
the chosen path.

### 2. Core owns the one-shot retry after auto-compaction

`run_turn` should keep a local `context_window_recovery_attempted` guard. On
the first `Err(CodexErr::ContextWindowExceeded)`, it runs:

```text
run_auto_compact(
    &sess,
    &turn_context,
    &mut client_session,
    InitialContextInjection::BeforeLastUserMessage,
    CompactionReason::ContextLimit,
    CompactionPhase::MidTurn,
)
```

If compaction succeeds, the loop sets `can_drain_pending_input = false` and
continues so the retried request uses the compacted context for the same turn.
If compaction fails, or if the retry returns `ContextWindowExceeded` again, the
turn reports the normal terminal error event with
`CodexErrorInfo::ContextWindowExceeded`.

The implementation must not add an unconditional client-session reset. If the
compaction path exposes a reset-required result, the turn loop should honor
that signal; otherwise it should preserve the existing incremental request
reuse behavior.

### 3. Admission estimation reuses local token estimates

Pre-sampling compaction should not require a Claude count-tokens request before
each turn. Instead, it should build or estimate the model-visible material that
will be added before sampling:

- context updates and reference-context reinjection recorded for the turn;
- the fresh user input;
- skill and plugin injection items;
- any pending items already recorded before request construction.

The estimate should reuse the existing local token-estimation helpers used by
`History` and in-flight context estimates. A conservative estimate is
acceptable because compacting slightly early is safer than sending a request
that immediately fails with a hard context-window overflow.

The admission guard should compare the projected total against both the
configured `model_auto_compact_token_limit` scope and the resolved
`model_context_window` where available. If the projected usage would exceed
either threshold, it should run pre-sampling auto-compaction before the request
is sent.

### 4. App surfaces remain fallback-only

The app-server/frontend context-window recovery flow should remain unchanged and
available for terminal failures. After this change, it is no longer the primary
Claude recovery path because core normally compacts and retries before the
terminal error reaches clients.

## Risks / Trade-offs

- Early compaction from conservative estimates -> keep the estimator simple,
  trace projected values, and prefer existing token-estimation helpers over new
  provider calls.
- Infinite retry loop after repeated overflow -> enforce the one-shot recovery
  guard and test repeated failure.
- Lost user steer during recovery -> set `can_drain_pending_input = false`
  after successful recovery so pending input waits until the current turn can
  resume.
- Hidden compaction failure -> propagate the original terminal
  `ContextWindowExceeded` when compaction fails due to another hard overflow,
  and keep usage-limit handling on the existing `UsageLimitReached` path.

## Migration Plan

1. Land Claude SSE error mapping and unit tests.
2. Land turn-loop one-shot recovery and mocked core success/failure tests.
3. Land admission estimation and targeted tests for projected overflow.
4. Run formatting, scoped fixes, targeted tests, and `cargo check`.

Rollback is straightforward: revert the Claude SSE error mapping and turn-loop
recovery branch while leaving unrelated Claude count-token and context-usage
behavior unchanged.

## Open Questions

- Whether `run_auto_compact` should expose an explicit reset-required result in
  this change, or whether existing remote/local compaction helpers already
  encapsulate the only required session-reset behavior.
