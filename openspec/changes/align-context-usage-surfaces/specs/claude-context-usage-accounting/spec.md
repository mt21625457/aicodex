## ADDED Requirements

### Requirement: Claude context usage surfaces share one occupancy snapshot

For providers routed through Claude Messages, Codex MUST derive context-window
occupancy from one canonical current-context usage snapshot represented by the
existing `TokenUsageInfo.context_tokens` / `ThreadTokenUsage.contextTokens`
contract when that value is available. Autocompact threshold checks,
blocking-limit checks, footer warnings, status-line context fields, and context
reports MUST use this snapshot or explicitly label a different metric as
non-occupancy usage.

#### Scenario: Footer warning matches threshold accounting

- **WHEN** a Claude turn appends tool results, user content, or other
  model-visible items after the most recent provider usage event
- **THEN** the footer context warning uses the canonical occupancy snapshot
  rather than only the most recent response usage
- **AND** the warning threshold agrees with the autocompact threshold decision

#### Scenario: Status line reports current occupancy

- **WHEN** status-line context-window data is generated for a Claude thread and
  `contextTokens` is present
- **THEN** the reported current context usage and percentage are based on that
  canonical occupancy snapshot
- **AND** cumulative input, output, cache, or billing-style counters are not
  presented as the current context occupancy unless they share the same
  semantics

#### Scenario: Client conversion preserves occupancy fields

- **WHEN** app-server emits `thread/tokenUsage/updated` with `contextTokens` and
  `contextSource`
- **THEN** clients that convert the payload into local token usage models
  preserve those fields or derive context display values before dropping them
- **AND** status-line or footer displays do not need to infer occupancy from
  cumulative `total` usage

#### Scenario: Context report headline matches canonical usage

- **WHEN** a context report or debug surface renders a Claude context report
  after request-time transforms have changed the model-visible history
- **THEN** the headline total and percentage use the canonical occupancy
  snapshot
- **AND** estimated category breakdowns are not allowed to replace or contradict
  the canonical headline value

### Requirement: Claude context snapshots account for request-time transforms

Codex MUST compute Claude context occupancy from the same model-visible request
view used for the next provider call after the Rust runtime's actual
compaction, replay, request normalization, and provider counting transforms have
been applied.

#### Scenario: Compaction reduces displayed occupancy

- **WHEN** local or remote compaction removes model-visible content before a
  Claude request
- **THEN** the context usage snapshot subtracts the freed content or recomputes
  from the transformed request view
- **AND** user-visible context displays do not continue to show stale
  pre-compaction usage

#### Scenario: Replay preserves restored occupancy

- **WHEN** a stored or forked Claude thread replays token usage with
  `contextTokens` and `contextSource`
- **THEN** the replayed occupancy is preserved in app-server notifications and
  client displays
- **AND** it is labeled with the replay source when replay is the reason for the
  value

#### Scenario: Post-response model-visible items are included

- **WHEN** model-visible items are added after the last provider usage event
- **THEN** the context usage snapshot includes every added item that will be
  sent on the next Claude request
- **AND** the count does not stop at the last provider usage event

### Requirement: Claude context usage distinguishes occupancy from spend

Codex MUST distinguish current context-window occupancy from cumulative token
spend and per-response completion usage. Missing or partial streamed usage MUST
NOT reset current occupancy for a non-empty Claude conversation when native
counting or local estimation can provide a value.

#### Scenario: Missing usage preserves non-empty occupancy

- **WHEN** a Claude-compatible provider omits streamed usage or reports zero
  input usage for a completed turn
- **THEN** Codex uses native count-tokens or local estimation for current
  context occupancy
- **AND** user-visible context displays do not show an empty context for a
  non-empty conversation

#### Scenario: Snapshot source is explainable

- **WHEN** context usage is computed from native count-tokens, API usage plus
  local estimate, or full local estimate
- **THEN** Codex records lightweight source metadata for debug or test
  assertions
- **AND** the metadata does not include raw prompts, tool inputs, credentials,
  media payloads, or provider-state payloads
