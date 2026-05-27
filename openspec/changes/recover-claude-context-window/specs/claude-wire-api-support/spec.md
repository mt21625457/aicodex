## ADDED Requirements

### Requirement: Claude context-window stop reason must become a provider-neutral error

Codex MUST treat Claude Messages
`stop_reason = "model_context_window_exceeded"` as a hard context-window
overflow. The Claude SSE adapter MUST return `ApiError::ContextWindowExceeded`
and MUST NOT emit a normal `ResponseEvent::Completed` for that stream.

#### Scenario: Claude stream reports model context window exceeded

- **WHEN** a Claude Messages stream receives a `message_delta` event with
  `stop_reason = "model_context_window_exceeded"`
- **THEN** the stream result is `ApiError::ContextWindowExceeded`
- **AND** no `ResponseEvent::Completed` is emitted for that stop reason

#### Scenario: Unknown Claude stop reason remains provider metadata

- **WHEN** a Claude Messages stream receives a valid but unknown stop reason
- **THEN** the stream completes successfully when the rest of the stream is
  valid
- **AND** the completed event preserves the raw provider stop reason
- **AND** the stream does not map the unknown reason to
  `ApiError::ContextWindowExceeded`
