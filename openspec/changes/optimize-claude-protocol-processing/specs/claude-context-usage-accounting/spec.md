## MODIFIED Requirements

### Requirement: Claude context usage uses native token counting

For providers routed through Claude Messages, Codex MUST use Anthropic's native
`/v1/messages/count_tokens` endpoint to refresh the current context-window usage
after a successful streaming turn when the endpoint is available. Completion
usage collected from the Claude stream MUST preserve non-zero input and cache
fields across stream events so the post-turn context refresh and completion
accounting do not regress when later stream deltas report zeroes.

#### Scenario: Zero-valued stream deltas do not erase input usage

- **WHEN** Claude sends `message_start.usage.input_tokens` with a non-zero value
- **AND** a later `message_delta.usage.input_tokens` is `0`
- **THEN** Codex preserves the non-zero input token value for completed stream
  usage
- **AND** context-window usage is not reset to zero for a non-empty
  conversation

#### Scenario: Zero-valued cache deltas do not erase cache usage

- **WHEN** Claude sends non-zero `cache_read_input_tokens` or
  `cache_creation_input_tokens` in an earlier stream event
- **AND** a later stream event reports the same field as `0`
- **THEN** Codex preserves the earlier non-zero value
- **AND** `cached_input_tokens` continues to represent cache-read input only

#### Scenario: Later non-zero usage can update earlier usage

- **WHEN** Claude sends an earlier usage event with an absent or lower cache
  field
- **AND** a later usage event reports a non-zero value for that field
- **THEN** Codex updates the completed stream usage to the later non-zero value
- **AND** the resulting `TokenUsage` remains internally consistent

### Requirement: Claude context counting degrades safely

Codex MUST treat count-token failures and incomplete streamed usage as
non-fatal accounting failures. If the Claude count-tokens endpoint is
unavailable, rejected, rate-limited, malformed, or not implemented by a
compatible provider, Codex MUST fall back to local context estimation and
continue the turn normally.

#### Scenario: Incomplete stream usage falls back without corrupting totals

- **WHEN** a Claude stream completes with missing or incomplete usage fields
- **AND** the native count-token refresh is unavailable
- **THEN** Codex uses local context estimation rather than emitting a misleading
  zero total
- **AND** the completed assistant response remains visible
