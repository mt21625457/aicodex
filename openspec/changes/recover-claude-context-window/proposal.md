## Why

Claude Messages streams can currently parse
`stop_reason = "model_context_window_exceeded"`, but the adapter still lets the
turn look like a normal completed assistant response. That prevents the shared
`ContextWindowExceeded` recovery path from reliably compacting context and
retrying the same turn, so Claude does not yet have parity with OpenAI
Responses when the model context window is full.

## What Changes

- Treat Claude `model_context_window_exceeded` as
  `ApiError::ContextWindowExceeded` instead of a normal
  `ResponseEvent::Completed`.
- Map the Claude adapter error through the existing
  `ApiError::ContextWindowExceeded -> CodexErr::ContextWindowExceeded ->
  CodexErrorInfo::ContextWindowExceeded` chain.
- Add a one-shot hard context-window recovery branch in the core turn loop:
  compact with `CompactionReason::ContextLimit`, retry the current turn, and
  avoid draining pending user steer before the model/tool continuation resumes.
- Report terminal `CodexErrorInfo::ContextWindowExceeded` only when compaction
  fails or the retry still exceeds the context window.
- Add conservative pre-sampling admission estimation for the pending turn input,
  context updates, and skill/plugin injection so Codex compacts before sending
  a request that is likely to overflow.
- Preserve app-server/frontend protocol shape and keep existing
  context-window fallback recovery as a final safety net, not the primary
  recovery path.

## Capabilities

### New Capabilities

- `context-window-recovery`: Codex can recover from provider-reported hard
  context-window overflow by compacting once and retrying the active turn before
  surfacing a terminal client error.

### Modified Capabilities

- `claude-wire-api-support`: Claude Messages stream handling changes the
  `model_context_window_exceeded` stop reason from a provider-stop metadata
  completion into the shared context-window error path.

## Impact

- Affected crates:
  - `codex-rs/codex-api` for Claude SSE stop-reason error mapping and tests.
  - `codex-rs/core` for turn-loop hard recovery, admission estimation, and
    mocked end-to-end recovery tests.
- Protocol compatibility:
  - No new app-server notifications.
  - No `thread/tokenUsage/updated` shape change.
  - No frontend IPC change.
  - OpenAI Responses behavior remains unchanged.
- Tool-loop compatibility:
  - Successful compaction resumes the same turn before consuming pending steer,
    so tool continuations and model follow-ups keep priority.
  - The retry guard allows at most one automatic recovery attempt for a single
    hard overflow.
- Primary risks:
  - Over-aggressive admission estimation could compact earlier than strictly
    necessary.
  - A failed compaction or second overflow must not loop forever or suppress the
    final `ContextWindowExceeded` error.
- Verification:
  - `cd codex-rs && cargo test -p codex-api claude`
  - `cd codex-rs && cargo test -p codex-core context_window`
  - `cd codex-rs && cargo check`
  - `cd codex-rs && just fmt`
  - `cd codex-rs && just fix -p codex-api`
  - `cd codex-rs && just fix -p codex-core`
