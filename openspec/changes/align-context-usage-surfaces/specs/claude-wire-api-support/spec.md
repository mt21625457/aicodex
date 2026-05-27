## ADDED Requirements

### Requirement: Claude context accounting uses the adapter-visible request view

For providers routed through Claude Messages, Codex MUST make context
occupancy accounting consume the same model-visible request view produced by
the Claude adapter for the next provider call, or an adapter-owned summary with
identical visibility semantics. UI, app-server, and status-line consumers MUST
receive the computed occupancy contract rather than reconstructing Claude
request transforms themselves.

#### Scenario: Count-token accounting follows serialized visibility

- **WHEN** Claude context usage is refreshed through native count-tokens or a
  provider-compatible fallback
- **THEN** the counted content matches the model-visible messages, tools,
  system content, and normalized blocks that the Claude adapter would send for
  the next provider request
- **AND** content removed or replaced by compaction, replay, or request
  normalization is not counted as current occupancy

#### Scenario: Client surfaces do not duplicate request transforms

- **WHEN** app-server or TUI surfaces render footer warnings, status-line
  context fields, or context reports for a Claude thread
- **THEN** they use the emitted context occupancy fields or derived display
  values from those fields
- **AND** they do not independently replay Claude request construction rules to
  infer current context occupancy
