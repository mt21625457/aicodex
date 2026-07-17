## ADDED Requirements

### Requirement: Provider configuration MUST support Chat Completions wire selection

Codex MUST allow model providers to select Chat Completions via `wire_api = "chat"`
(mapped to `WireApi::Chat`). Deserializing this value MUST succeed. The default wire API
MUST remain OpenAI Responses. Documentation MUST describe when to choose `responses`,
`claude` / `anthropic`, and `chat`.

#### Scenario: Config selects chat wire API

- **WHEN** a provider config sets `wire_api = "chat"`
- **THEN** Codex deserializes the provider as `WireApi::Chat`
- **AND** sampling dispatch uses the Chat Completions adapter path

#### Scenario: Default remains Responses

- **WHEN** a provider config omits `wire_api`
- **THEN** Codex uses `WireApi::Responses`
- **AND** existing Responses providers keep their previous routing behavior

#### Scenario: Legacy removal error is retired for chat

- **WHEN** users migrate from older builds that rejected `wire_api = "chat"`
- **THEN** current Codex accepts the chat value
- **AND** docs no longer instruct users that chat is permanently unsupported

### Requirement: Remote thread config MUST retain its existing wire API boundary

This change MUST NOT extend the managed/remote thread-config `WireApi` proto enum with
`WIRE_API_CHAT`. Remote thread configs that carry a wire API value outside the existing proto
enum MUST continue to fail with a parse error. Chat selection is available through local model
provider configuration only in this change.

#### Scenario: Local chat support does not expand the remote proto

- **WHEN** local provider configuration accepts `wire_api = "chat"`
- **THEN** the managed/remote thread-config proto remains unchanged
- **AND** it does not expose a `WIRE_API_CHAT` enum value as part of this change

#### Scenario: Remote config with unknown wire API remains an error

- **WHEN** a managed/remote thread config delivers a provider wire API value outside the proto enum
- **THEN** Codex rejects it with the existing unknown-wire-api parse error
