## ADDED Requirements

### Requirement: Claude Messages MUST advertise file tools according to the selected policy

When the `dedicated_file_tools` rollout gate is disabled, Claude Messages MUST
preserve the existing model-visible and dispatch plans. When it is enabled, the
typed feature-config `mode` MUST resolve to the finite policy described below;
omission defaults to `auto`. Dedicated tools MUST be
ordinary Claude function tools with JSON `input_schema` values that preserve
required fields and descriptions.

#### Scenario: disabled gate restores the prior dispatch plan

- **WHEN** `[features] dedicated_file_tools = false`, the object config has
  `enabled = false`, or the feature is absent
- **THEN** `read_file`, `edit_file`, and `write_file` are absent from the Claude
  request and the tool registry
- **AND** a forged call to one of those names returns unsupported-tool without
  filesystem access

#### Scenario: feature config rejects an unknown mode

- **WHEN** `features.dedicated_file_tools.mode` is not `auto`, `dedicated`, or
  `dedicated_with_apply_patch`, or the object contains an unknown field
- **THEN** config loading fails with a field-specific error
- **AND** Codex does not silently fall back to another tool plan

#### Scenario: auto mode preserves the Anthropic native editor as the sole primary editor

- **WHEN** an Anthropic Claude turn enables the rollout gate in `auto` mode
- **THEN** the `/v1/messages` request advertises the supported native
  `text_editor_*` tool
- **AND** it does not advertise `read_file`, `edit_file`, `write_file`, or
  `apply_patch`
- **AND** the underlying native editor executor remains backed by the shared
  reviewable mutation runtime

#### Scenario: Compatible Claude provider gets dedicated file tools without native text_editor

- **WHEN** a Claude Compatible provider turn enables the rollout gate in `auto`
  or `dedicated` mode
- **THEN** the request advertises `read_file`, `edit_file`, and `write_file`
- **AND** the request does not advertise Anthropic-only `text_editor_*` tool
  types
- **AND** each dedicated entry is a function-style tool with an object
  `input_schema`

#### Scenario: Kimi K3 auto mode uses dedicated tools

- **WHEN** a Claude-wire turn uses a model recognized as Kimi K3 with the
  rollout gate enabled in `auto` mode
- **THEN** the request advertises `read_file`, `edit_file`, and `write_file`
- **AND** it does not advertise native `text_editor_*` or `apply_patch`
- **AND** each dedicated entry remains an ordinary JSON function tool

#### Scenario: Anthropic dedicated mode opts into function file tools

- **WHEN** an Anthropic turn explicitly selects `dedicated` mode
- **THEN** the request advertises `read_file`, `edit_file`, and `write_file`
- **AND** it does not advertise native `text_editor_*` or `apply_patch`

### Requirement: Claude MUST collapse competing file-edit surfaces when dedicated tools are enabled

When dedicated file tools are model-visible on a Claude turn, Codex MUST NOT
also advertise Anthropic native `text_editor` tools. Codex MUST advertise
`apply_patch` only in the explicit `dedicated_with_apply_patch` mode. In all
other rollout-enabled modes, pre-existing non-visible legacy handlers MAY remain
registered for hidden dispatch compatibility. That legacy exception does not
keep the three new dedicated names registered after their gate is disabled.

#### Scenario: dedicated tools suppress native text_editor advertisement

- **WHEN** dedicated file tools are enabled on a Claude Anthropic turn
- **THEN** the request tools list does not include a tool whose `type` starts
  with `text_editor_`
- **AND** `read_file`, `edit_file`, and `write_file` remain present

#### Scenario: dedicated tools suppress apply_patch unless fallback mode selected

- **WHEN** dedicated file tools are enabled in `dedicated` or Compatible `auto`
  mode
- **THEN** the request tools list does not include `apply_patch`
- **AND** file mutations are expected through `edit_file` or `write_file`

#### Scenario: explicit fallback mode can keep apply_patch visible

- **WHEN** `dedicated_with_apply_patch` is selected
- **THEN** the request includes `apply_patch` in addition to dedicated file
  tools
- **AND** native `text_editor_*` types remain omitted

#### Scenario: hidden handlers support resumed legacy calls without being advertised

- **WHEN** a resumed transcript contains a valid legacy `apply_patch` or native
  editor call while the current mode hides that tool
- **THEN** Codex can dispatch the call through its registered compatibility
  handler
- **AND** the hidden tool is absent from the new request's model-visible tools

### Requirement: Claude tool loop MUST execute dedicated file tools end to end

Codex MUST map Claude `tool_use` names for `read_file`, `edit_file`, and
`write_file` back to the corresponding handlers, execute them through the
dedicated-file-tools safety rules, and return Claude `tool_result` blocks that
allow the turn to continue.

#### Scenario: Claude edit_file tool_use mutates a workspace file

- **WHEN** a Claude stream issues `tool_use` for `edit_file` with valid
  arguments after a successful `read_file` of the same path in an earlier
  provider response within the same user turn
- **THEN** Codex executes the edit handler
- **AND** the follow-up Claude request includes a non-error `tool_result` for
  that `tool_use` id
- **AND** the target file contents reflect the edit

#### Scenario: multi-environment tool_use selects the matching receipt namespace

- **WHEN** a Claude tool call supplies `environment_id` for one of multiple
  selected environments
- **THEN** the handler resolves the path and receipt in that environment only
- **AND** a read receipt from another environment cannot authorize the mutation
