## ADDED Requirements

### Requirement: Claude post-turn accounting remains adapter-scoped

Claude post-turn context usage refresh MUST stay within the Claude Messages
protocol boundary. The implementation MUST reuse Claude request serialization,
Claude endpoint authentication headers, and Claude endpoint paths without
adding Claude-specific behavior to the OpenAI Responses request builder or SSE
parser.

#### Scenario: Claude count request uses Claude endpoint semantics

- **WHEN** Codex refreshes context usage for a Claude Messages provider
- **THEN** the count request is sent to `/v1/messages/count_tokens`
- **AND** the request includes Claude authentication and version headers through
  the Claude endpoint adapter
- **AND** the request body reuses the same Claude Messages serialization rules
  that normal Claude turns use for model, system, messages, tools, tool choice,
  thinking, and service tier

#### Scenario: Claude tool-loop semantics are unchanged

- **WHEN** a Claude Messages turn uses client-side tools, pause-turn
  continuation, or ordinary assistant text
- **THEN** context usage refresh MUST NOT reorder messages, change tool result
  handling, or alter the streamed response items emitted to Codex
