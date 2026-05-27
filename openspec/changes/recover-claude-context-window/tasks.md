## 1. Claude SSE Error Mapping

- [x] 1.1 Update `codex-rs/codex-api/src/sse/claude.rs` so
  `ClaudeStopReason::ModelContextWindowExceeded` returns
  `ApiError::ContextWindowExceeded`.
- [x] 1.2 Ensure the same stream cannot emit `ResponseEvent::Completed` with
  `end_turn = true` for `model_context_window_exceeded`.
- [x] 1.3 Add a codex-api Claude SSE unit test where
  `message_delta.stop_reason = "model_context_window_exceeded"` yields
  `ApiError::ContextWindowExceeded`.
- [x] 1.4 Recheck unknown Claude stop reasons still parse and complete as
  provider-stop metadata.

## 2. Core Hard Context Recovery

- [x] 2.1 Add a one-shot `context_window_recovery_attempted` guard in
  `codex-rs/core/src/session/turn.rs`.
- [x] 2.2 On first `Err(CodexErr::ContextWindowExceeded)`, run mid-turn
  `run_auto_compact` with `InitialContextInjection::BeforeLastUserMessage`,
  `CompactionReason::ContextLimit`, and `CompactionPhase::MidTurn`.
- [x] 2.3 After successful recovery, set `can_drain_pending_input = false` and
  retry the current turn.
- [x] 2.4 Preserve existing `UsageLimitReached`, `TurnAborted`, and invalid
  image handling semantics.
- [x] 2.5 Do not add unconditional `reset_client_session`; only reset if the
  compaction result or existing incremental-check logic requires it.

## 3. Pre-Sampling Admission Guard

- [x] 3.1 Replace the pre-turn compaction TODO with a conservative projected
  token estimate for the material that will be visible in the next sampling
  request.
- [x] 3.2 Include fresh user input, recorded context updates, and skill/plugin
  injection items in the projected estimate.
- [x] 3.3 Reuse existing local token-estimation helpers instead of adding a
  mandatory Claude `/messages/count_tokens` preflight.
- [x] 3.4 Trigger pre-sampling `run_auto_compact` when projected usage exceeds
  the configured auto-compact threshold or resolved model context window.
- [x] 3.5 Add trace fields for active, projected, and threshold token values.

## 4. Core Tests

- [x] 4.1 Add a codex-core mocked Claude turn test where the first request
  returns `ContextWindowExceeded`, core auto-compacts, retry succeeds, and the
  turn completes without terminal `CodexErrorInfo::ContextWindowExceeded`.
- [x] 4.2 Assert the compact path was invoked in the successful recovery test.
- [x] 4.3 Add a repeated-overflow test where first request overflows, compact
  runs, retry overflows again, and the terminal event contains
  `CodexErrorInfo::ContextWindowExceeded`.
- [x] 4.4 Add a compact-failure test where overflow-triggered compaction fails
  and the terminal event contains `CodexErrorInfo::ContextWindowExceeded`.
- [x] 4.5 Assert the failure tests do not loop and do not issue more than one
  automatic recovery attempt.
- [x] 4.6 Add or update pre-sampling admission tests for projected input that
  crosses the auto-compact threshold before the provider request is sent.

## 5. Protocol Stability

- [x] 5.1 Confirm no app-server notification or frontend IPC payload is added.
- [x] 5.2 Confirm `thread/tokenUsage/updated` shape is unchanged.
- [x] 5.3 Keep existing app-side context-window fallback tests unchanged unless
  they assert a now-earlier core recovery path.
- [x] 5.4 Confirm OpenAI Responses context-window behavior remains unchanged.

## 6. Verification

- [x] 6.1 Run `cd codex-rs && just test -p codex-api claude`.
- [x] 6.2 Run `cd codex-rs && just test -p codex-core context_window`.
- [x] 6.3 Run scoped `cd codex-rs && cargo check -p codex-api -p codex-core`.
  Workspace `cargo check` was attempted but blocked by a `webrtc-sys`
  download timeout, so full workspace compile is left for local verification.
- [x] 6.4 Run `cd codex-rs && just fmt`.
- [x] 6.5 Run `cd codex-rs && just fix -p codex-api`.
- [x] 6.6 Run `cd codex-rs && just fix -p codex-core`.
- [x] 6.7 Run `openspec validate recover-claude-context-window --strict`.
