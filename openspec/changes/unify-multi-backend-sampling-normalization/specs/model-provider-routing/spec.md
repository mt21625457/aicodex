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

### Requirement: Remote thread config MUST support Chat Completions wire selection

Codex MUST accept `WIRE_API_CHAT` in the managed/remote thread-config `WireApi` proto enum and
map it to `WireApi::Chat`. Remote thread configs that carry an unknown wire API value MUST
continue to fail with a parse error.

#### Scenario: Remote config selects chat wire API

- **WHEN** a managed/remote thread config delivers a provider with `wire_api = WIRE_API_CHAT`
- **THEN** Codex deserializes the provider as `WireApi::Chat`
- **AND** sampling dispatch uses the Chat Completions adapter path

#### Scenario: Remote config with unknown wire API remains an error

- **WHEN** a managed/remote thread config delivers a provider wire API value outside the proto enum
- **THEN** Codex rejects it with the existing unknown-wire-api parse error
