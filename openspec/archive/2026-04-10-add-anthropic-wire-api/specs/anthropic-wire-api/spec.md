# Spec Delta: Anthropic Wire API

## ADDED Requirements

### Requirement: Provider configuration may declare Anthropic wire protocol

The system MUST allow a model provider to declare `wire_api = "anthropic"` in provider configuration.

#### Scenario: Anthropic provider is selected from config

- **GIVEN** a provider entry with `wire_api = "anthropic"`
- **WHEN** that provider is selected for a turn
- **THEN** the runtime MUST recognize it as a distinct wire protocol from `responses`

### Requirement: Anthropic protocol logic must live in a dedicated workspace crate

Anthropic protocol execution MUST be implemented in a dedicated workspace crate under a new folder, rather than embedding the full implementation directly inside `codex-core`.

#### Scenario: Anthropic implementation is added

- **WHEN** Anthropic support is introduced
- **THEN** the bulk of request building, stream decoding, and error mapping MUST live in `codex-rs/anthropic/`
- **AND** `codex-core` changes MUST be limited to thin integration and dispatch

### Requirement: New Anthropic code must be concentrated in one folder

New Anthropic support code MUST be added under a single implementation folder by default, with only narrowly scoped exceptions for required integration points in existing code.

#### Scenario: New Anthropic implementation code is added

- **WHEN** a new Anthropic-related source file is introduced
- **THEN** it MUST be placed under `codex-rs/anthropic/`
- **AND** edits outside that folder MUST be limited to required integration files or required config/schema/doc exposure

### Requirement: Anthropic adapter must reuse the existing provider framework

The Anthropic implementation MUST reuse the existing provider framework for provider selection and provider configuration.

#### Scenario: Anthropic provider uses existing config fields

- **GIVEN** an Anthropic provider definition
- **WHEN** the runtime prepares a turn request
- **THEN** it MUST reuse existing provider fields for base URL, auth/env lookup, HTTP headers, retries, and timeouts

### Requirement: Anthropic stream output must map into existing internal response shapes

The Anthropic adapter MUST map stream output into the existing internal response and event model used by the rest of the system.

#### Scenario: Anthropic assistant text is streamed

- **GIVEN** an Anthropic streaming response with assistant text deltas
- **WHEN** the adapter processes the stream
- **THEN** it MUST emit internal text delta events compatible with existing turn handling

#### Scenario: Anthropic tool use is streamed

- **GIVEN** an Anthropic streaming response containing tool use content
- **WHEN** the adapter completes the tool-use item
- **THEN** it MUST emit internal tool-call response items compatible with the existing tool runtime

### Requirement: Existing orchestration and persistence flows remain unchanged

Anthropic support MUST integrate with the current session, tool runtime, rollout, and state persistence pipeline without introducing a parallel orchestration system.

#### Scenario: Anthropic turn completes

- **GIVEN** a turn executed against an Anthropic provider
- **WHEN** the turn finishes
- **THEN** the resulting items MUST flow through the same follow-up, persistence, and UI-event handling path used for existing providers

### Requirement: Unsupported Anthropic features must be explicit

Features not implemented in the current Anthropic phase MUST be either clearly documented as unsupported or rejected explicitly at runtime.

#### Scenario: Unsupported Anthropic feature is requested

- **GIVEN** an Anthropic flow reaches a feature outside current support
- **WHEN** the runtime cannot faithfully execute it
- **THEN** the system MUST not silently pretend support exists
- **AND** project documentation for the current phase MUST identify that limitation
