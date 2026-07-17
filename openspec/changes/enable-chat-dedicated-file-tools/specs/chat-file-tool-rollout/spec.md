## ADDED Requirements

### Requirement: Chat dedicated file tools MUST use an explicit fail-closed rollout policy

Codex MUST model Chat file-tool selection as a typed mode with exactly
`legacy`, `dedicated`, and `dedicated_with_apply_patch` behavior. The default
MUST be `legacy`. Non-legacy modes MUST require the dedicated-file-tools safety
foundation and MUST fail closed when that dependency is unavailable.

#### Scenario: omitted mode preserves legacy behavior

- **WHEN** a Chat provider session omits `chat_file_tool_mode`
- **THEN** Codex uses `legacy`
- **AND** the Chat model-visible tool plan is unchanged by this capability

#### Scenario: dedicated mode exposes one file-edit surface

- **WHEN** a Chat provider session selects `dedicated`
- **THEN** `read_file`, `edit_file`, and `write_file` are model-visible Chat
  function declarations
- **AND** `apply_patch` remains registered for hidden dispatch but is not
  model-visible

#### Scenario: explicit fallback mode exposes apply_patch

- **WHEN** a Chat provider session selects `dedicated_with_apply_patch`
- **THEN** all three dedicated file tools and `apply_patch` are model-visible
- **AND** the mode is not selected implicitly

#### Scenario: unavailable foundation fails closed

- **WHEN** a non-legacy Chat mode is configured but the dedicated handlers or
  safety foundation are unavailable
- **THEN** Codex rejects configuration or session startup with an actionable
  dependency error
- **AND** Codex does not silently send a legacy sampling request

### Requirement: Chat file-tool rollout MUST remain isolated from other wires

`chat_file_tool_mode` MUST affect only providers using `WireApi::Chat`.
Responses and Claude tool selection MUST remain governed by their existing
policies, and changing the Chat mode MUST NOT rewrite conversation history.

#### Scenario: Chat dedicated mode does not affect Responses or Claude

- **WHEN** Chat dedicated mode is configured and equivalent Responses or Claude
  sessions build requests
- **THEN** their model-visible tool lists remain unchanged
- **AND** no Chat mapped-name guidance appears in those requests

#### Scenario: mode change applies only to a new session plan

- **WHEN** a user changes Chat mode for a new session
- **THEN** the new tool plan reflects the selected mode
- **AND** previously stored assistant tool calls and outputs are not rewritten

### Requirement: Chat dedicated guidance MUST reference callable mapped tools

When dedicated file tools are model-visible, Codex MUST derive bounded Chat
guidance from the actual serialized tool declarations and reverse metadata. The
guidance MUST unambiguously reference each callable mapped read/edit/write tool,
MUST require dependent Read then Edit/Write operations to occur across
completions, and MUST preserve documented specialized shell fallbacks.

#### Scenario: guidance uses actual mapped names

- **WHEN** Chat dedicated tools serialize to stable hashed wire names
- **THEN** the system guidance references those callable mapped names or an
  equally unambiguous mapping present in the request
- **AND** it does not instruct the model to call only an undeclared bare name

#### Scenario: incomplete declaration set fails request construction

- **WHEN** reverse metadata cannot uniquely resolve all three visible dedicated
  file tools
- **THEN** Chat request construction fails before sampling
- **AND** Codex does not emit misleading dedicated-tool guidance

#### Scenario: legacy mode omits dedicated guidance

- **WHEN** Chat mode is `legacy`
- **THEN** no guidance claims that dedicated file tools are available

### Requirement: Chat provider incompatibility MUST NOT trigger automatic legacy replay

Codex MUST surface the existing Chat error with mode-aware remediation when a
Chat-compatible provider rejects tool declarations, schemas, tool choice, or
tool-call streaming in a non-legacy mode. It MUST NOT automatically replay the
turn in legacy mode.

#### Scenario: provider rejects dedicated tool schema

- **WHEN** a provider returns an HTTP or stream error for a dedicated Chat
  request
- **THEN** Codex reports an actionable error that identifies the selected mode
  and suggests `legacy` rollback
- **AND** no automatic second sampling request is sent
