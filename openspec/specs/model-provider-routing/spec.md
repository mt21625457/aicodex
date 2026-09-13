# model-provider-routing Specification

## Purpose

Keep model inference aligned with upstream OpenAI Codex by supporting only the OpenAI Responses API.

## Requirements

### Requirement: Provider configuration MUST use Responses

The only supported model wire protocol is `responses`. Omitting `wire_api` MUST select Responses.
Local configurations selecting `chat`, `claude`, or the former `anthropic` alias MUST fail with an actionable message to configure a Responses-compatible endpoint. Changing the protocol value alone does not convert a Chat Completions or Messages endpoint into a Responses endpoint.

#### Scenario: Responses provider selection

- **WHEN** a provider config selects `responses` or omits `wire_api`
- **THEN** inference uses the Responses HTTP or WebSocket transport according to the provider capabilities
- **AND** provider-specific credentials and request state remain isolated when a turn selects another Responses provider

#### Scenario: Retired protocol configuration

- **WHEN** a provider config selects `chat`, `claude`, or `anthropic`
- **THEN** configuration loading fails with Responses migration guidance
- **AND** no Chat Completions or Claude Messages request is issued

### Requirement: Remote configuration MUST enforce the same protocol boundary

Legacy protobuf enum values and field numbers remain reserved for compatibility with stored data. Receiving a retired protocol MUST fail during remote configuration conversion instead of silently changing its endpoint or credentials.

#### Scenario: Remote retired protocol

- **WHEN** a remote provider contains the former Chat or Claude protocol value
- **THEN** configuration loading reports that Responses is required

### Requirement: Task metadata MUST use provider configuration

Protocol metadata MUST come from the configured provider. Model names and provider-name substrings MUST NOT be used to infer a retired protocol. Historical transcript data may still be read without re-enabling its former transport.
