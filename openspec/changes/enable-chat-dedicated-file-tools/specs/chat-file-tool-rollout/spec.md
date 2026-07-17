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

- **WHEN** a non-legacy Chat mode is resolved for a new Chat session while the
  `dedicated_file_tools` gate is disabled
- **THEN** Codex rejects the resolved session configuration with an actionable
  dependency error identifying the selected mode and `legacy` rollback
- **AND** Codex does not silently send a legacy sampling request

#### Scenario: incomplete runtime plan fails before HTTP sampling

- **WHEN** a non-legacy Chat session reaches request construction but any
  dedicated runtime, visible declaration, or reverse mapping is missing
- **THEN** Codex fails request construction before making the first provider HTTP
  call
- **AND** the error identifies the missing dependency and suggests `legacy`
  rollback

#### Scenario: non-Chat session ignores the Chat rollout field

- **WHEN** a Responses or Claude session carries a non-legacy
  `chat_file_tool_mode` while the Chat dependency gate is disabled
- **THEN** the session retains its existing tool policy
- **AND** it does not fail solely because the Chat-only gate is disabled

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
- **AND** an already-running session does not switch mode at a turn boundary
- **AND** previously stored assistant tool calls and outputs are not rewritten

### Requirement: Chat dedicated guidance MUST reference callable mapped tools

When dedicated file tools are model-visible, Codex MUST derive bounded Chat
guidance from the actual serialized tool declarations and reverse metadata. The
guidance MUST unambiguously reference each callable mapped read/edit/write tool,
MUST require dependent Read then Edit/Write operations to occur across
completions, and MUST preserve documented specialized shell fallbacks. The
guidance MUST be represented by a fixed-marker `ContextualUserFragment` under
`core/context`, MUST have a hard limit below 1K tokens, and MUST require both an
explicit non-legacy mode and matching first-party serialized metadata.

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

#### Scenario: same-named third-party tools do not activate guidance

- **WHEN** Chat mode is `legacy` and dynamic or third-party tools use semantic
  names `read_file`, `edit_file`, and `write_file`
- **THEN** Codex does not infer rollout mode from those names
- **AND** no dedicated Chat guidance is injected

#### Scenario: guidance remains stable and bounded within the session

- **WHEN** a non-legacy Chat session retries or performs later sampling steps
- **THEN** the rendered guidance stays below its hard token cap and is
  deep-equal for the same resolved tool identities
- **AND** Codex does not append duplicate guidance fragments to conversation
  history

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
