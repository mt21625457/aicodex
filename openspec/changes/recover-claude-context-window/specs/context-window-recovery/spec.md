## ADDED Requirements

### Requirement: Core must auto-compact and retry once after hard context overflow

Codex MUST attempt automatic context recovery before reporting a terminal
context-window error to clients when a sampling request returns
`CodexErr::ContextWindowExceeded`. The recovery attempt MUST run auto-compaction
with `CompactionReason::ContextLimit`, retry the current turn at most once, and
surface `CodexErrorInfo::ContextWindowExceeded` only if compaction fails or the
retry still exceeds the context window.

#### Scenario: Overflow recovers after compaction

- **WHEN** the first sampling request for a turn returns
  `CodexErr::ContextWindowExceeded`
- **AND** automatic compaction succeeds
- **AND** the retried sampling request succeeds
- **THEN** the turn completes normally
- **AND** clients do not receive a terminal
  `CodexErrorInfo::ContextWindowExceeded`

#### Scenario: Repeated overflow reports terminal error

- **WHEN** the first sampling request for a turn returns
  `CodexErr::ContextWindowExceeded`
- **AND** automatic compaction succeeds
- **AND** the retried sampling request also returns
  `CodexErr::ContextWindowExceeded`
- **THEN** Codex reports a terminal error event with
  `CodexErrorInfo::ContextWindowExceeded`
- **AND** Codex does not attempt a second automatic recovery for the same turn

#### Scenario: Compaction failure reports terminal error

- **WHEN** a sampling request returns `CodexErr::ContextWindowExceeded`
- **AND** automatic compaction fails
- **THEN** Codex reports a terminal error event with
  `CodexErrorInfo::ContextWindowExceeded`
- **AND** existing usage-limit handling remains on the
  `CodexErrorInfo::UsageLimitExceeded` path when the compaction failure is a
  usage-limit failure

### Requirement: Context-window recovery must preserve current-turn priority

After a successful hard-overflow compaction, Codex MUST retry the current model
request before draining pending user input that arrived while the model was
running.

#### Scenario: Pending steer waits until retry

- **WHEN** a turn receives pending user input while the first sampling request
  is running
- **AND** the sampling request returns `CodexErr::ContextWindowExceeded`
- **AND** automatic compaction succeeds
- **THEN** Codex retries the compacted current turn before recording the
  pending input into the model-visible request

### Requirement: Pre-sampling admission must estimate incoming turn material

Before sending a sampling request, Codex MUST conservatively estimate whether
the next model-visible request will exceed the configured auto-compact limit or
the resolved model context window after adding the current turn's incoming
material. This estimate MUST reuse local token-estimation mechanisms and MUST
NOT require a Claude `/messages/count_tokens` preflight for every request.

#### Scenario: Projected usage crosses auto-compact threshold

- **WHEN** the existing context is below the current auto-compact threshold
- **AND** the fresh user input, context updates, or skill/plugin injection
  would push the next request over that threshold
- **THEN** Codex runs pre-sampling auto-compaction before sending the provider
  request

#### Scenario: Claude preflight count is not required

- **WHEN** the active provider is routed through Claude Messages
- **THEN** Codex uses local projected-token estimation for admission decisions
- **AND** Codex does not require `/messages/count_tokens` to succeed before
  every sampling request

### Requirement: Context recovery must not change public app protocol shape

Codex MUST preserve existing app-server and frontend protocol shapes while
adding core context-window recovery. Existing app-side context recovery remains
available only as a fallback for terminal failures.

#### Scenario: Terminal error uses existing error surface

- **WHEN** auto-compaction fails or the one retry still exceeds the context
  window
- **THEN** Codex reports the existing error event with
  `CodexErrorInfo::ContextWindowExceeded`
- **AND** no new app-server notification or frontend IPC payload is required

#### Scenario: Token usage notification shape is unchanged

- **WHEN** context-window recovery succeeds or fails
- **THEN** `thread/tokenUsage/updated` keeps its existing payload shape
