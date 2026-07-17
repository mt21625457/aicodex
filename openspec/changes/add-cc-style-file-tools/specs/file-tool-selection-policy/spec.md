## ADDED Requirements

### Requirement: Sessions with dedicated file tools MUST prefer them over shell file IO

When `read_file`, `edit_file`, and `write_file` are advertised, Codex MUST
include model-facing guidance that ordinary file reads, edits, and creates use
those tools instead of shell commands such as `cat`, `head`, `tail`, `sed`,
heredoc redirection, `Get-Content`, `Set-Content`, or `Out-File`.
The guidance MUST permit shell or a specialized script after a dedicated tool
explicitly reports that a file exceeds its text, encoding, or editable-size
contract.

#### Scenario: tool-usage guidance mentions dedicated file tools

- **WHEN** a turn advertises dedicated file tools
- **THEN** the model-facing instructions include explicit prefer-dedicated-file
  tools over shell file IO guidance
- **AND** the guidance names `read_file`, `edit_file`, and `write_file`

#### Scenario: guidance is absent when dedicated tools are not visible

- **WHEN** a turn does not advertise all three dedicated file tools
- **THEN** Codex does not inject instructions claiming those tools are available

#### Scenario: dedicated capability error permits specialized fallback

- **WHEN** a dedicated file tool reports that the target is binary, unsupported
  text, or above the editable byte cap
- **THEN** the model-facing error permits an appropriate shell/script fallback
- **AND** ordinary supported text operations remain directed to dedicated tools

### Requirement: Model-visible shell descriptions MUST NOT steer simple file IO

Every model-visible shell surface MUST NOT present shell commands as the
preferred way to read or write ordinary workspace files when dedicated file
tools are visible. This includes `exec_command`, `shell_command`, and any
selected Claude native bash surface. Platform-specific wording MUST describe
the selected target environment rather than relying only on the core host
compile target.

#### Scenario: remote Windows shell description discourages PowerShell file IO

- **WHEN** Codex builds the model request for a remote Windows executor and
  dedicated file tools are visible
- **THEN** every advertised shell surface and/or the shared system guidance
  tells the model not to use PowerShell for ordinary file read/write/edit
- **AND** the description still may document PowerShell syntax for genuine
  shell or process operations

#### Scenario: unified exec receives the same selection guidance

- **WHEN** `exec_command` is model-visible instead of `shell_command` and
  dedicated file tools are visible
- **THEN** the request still contains the prefer-dedicated-file-tools guidance

### Requirement: OpenAI Responses edit surface remains apply_patch in this change

Codex MUST keep `apply_patch` as the model-visible file-edit tool on the
Responses wire. The `dedicated_file_tools` rollout defined by this change MUST
NOT advertise `read_file`, `edit_file`, or `write_file` on Responses. A future
Responses rollout requires a separate proposal.

#### Scenario: Responses turn keeps apply_patch primary

- **WHEN** a Responses-wire turn runs regardless of the Claude dedicated rollout
- **THEN** Codex continues to advertise `apply_patch` for file editing according
  to existing model or provider defaults
- **AND** `read_file` / `edit_file` / `write_file` do not appear because of this
  change
