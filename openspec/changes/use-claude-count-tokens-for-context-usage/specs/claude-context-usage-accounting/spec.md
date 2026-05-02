## ADDED Requirements

### Requirement: Claude context usage uses native token counting

For providers routed through Claude Messages, Codex MUST use Anthropic's native
`/v1/messages/count_tokens` endpoint to refresh the current context-window usage
after a successful streaming turn when the endpoint is available. The refreshed
usage MUST be emitted through the existing `TokenCount` and
`thread/tokenUsage/updated` notification path.

#### Scenario: Successful native count refresh

- **WHEN** a Claude Messages turn completes and the count-tokens endpoint
  returns an input token count
- **THEN** Codex updates the thread token usage notification so
  `last.totalTokens` reflects the native count of the current model-visible
  context
- **AND** the notification preserves the resolved model context window

#### Scenario: Missing streamed usage does not reset context usage

- **WHEN** a Claude Messages streaming response omits usage or reports zero
  input usage
- **THEN** Codex MUST NOT emit a zero context-window usage value for a non-empty
  conversation when native count-tokens or fallback estimation can provide a
  count

### Requirement: Claude context counting degrades safely

Codex MUST treat count-token failures as non-fatal accounting failures. If the
Claude count-tokens endpoint is unavailable, rejected, rate-limited, malformed,
or not implemented by a compatible provider, Codex MUST fall back to local
context estimation and continue the turn normally.

#### Scenario: Count-token endpoint failure uses local estimate

- **WHEN** a Claude Messages turn completes and `/v1/messages/count_tokens`
  returns an error or cannot be parsed
- **THEN** Codex emits a token usage update based on the existing local context
  estimator
- **AND** the completed assistant response remains visible and the turn is not
  failed solely because the accounting refresh failed

#### Scenario: OpenAI Responses accounting is unaffected

- **WHEN** a provider is routed through OpenAI Responses
- **THEN** Codex MUST NOT call the Claude count-tokens endpoint
- **AND** existing Responses completion usage behavior remains unchanged
