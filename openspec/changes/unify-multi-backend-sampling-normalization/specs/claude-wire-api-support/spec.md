## ADDED Requirements

### Requirement: Claude Messages adapter MUST act as an L2 ResponseEvent transform

Codex MUST keep Claude Messages wire behavior inside the Claude adapter modules while
conforming to the shared multi-backend L2 contract. Claude requests MUST continue to
target `/v1/messages` and MUST NOT be routed through Chat Completions or OpenAI
Responses parsers. Claude L2 MUST emit `ResponseEvent` values for turn orchestration and
MUST honor content-aware idle semantics for non-meaningful keepalive frames.

#### Scenario: Claude remains on Messages wire

- **WHEN** a provider is configured with `WireApi::Claude`
- **THEN** Codex continues to send Claude Messages requests to `/v1/messages`
- **AND** Claude stream parsing remains in the Claude-specific SSE module

#### Scenario: Claude participates in shared L2 idle contract

- **WHEN** a Claude stream delivers only non-meaningful keepalive-style frames beyond the idle budget
- **THEN** Codex surfaces an idle/timeout style stream error
- **AND** those frames alone do not count as meaningful progress

#### Scenario: Claude isolation from Chat Completions

- **WHEN** Chat Completions support is added
- **THEN** Claude request construction and SSE accumulation do not gain Chat chunk parsing branches
- **AND** Chat Completions support does not require edits to Claude history serialization rules
