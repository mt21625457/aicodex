## ADDED Requirements

### Requirement: Chat Completions MUST preserve dedicated file-tool identity through mapped names

Codex MUST serialize `read_file`, `edit_file`, and `write_file` using the
existing deterministic Chat function-name mapping and MUST preserve reverse
metadata to recover their semantic Codex function identities. This rollout MUST
NOT special-case or rewrite the established Chat hashing/sanitization contract.

#### Scenario: dedicated functions receive stable Chat wire names

- **WHEN** Chat serializes the three dedicated file tools
- **THEN** each declaration is a function tool whose name equals the existing
  `chat_tool_name` result for that semantic function identity
- **AND** reordering tools or changing ApplyPatch visibility does not change
  those names

#### Scenario: streamed mapped call recovers semantic identity

- **WHEN** Chat SSE streams a mapped dedicated function name with fragmented
  JSON arguments
- **THEN** Codex reconstructs a `ResponseItem::FunctionCall` for the matching
  semantic `read_file`, `edit_file`, or `write_file`
- **AND** the existing handler executes the call through the
  dedicated-file-tools safety contract

### Requirement: Chat dedicated tool results MUST continue with valid tool-role history

After a dedicated file-tool call executes, Codex MUST serialize the next Chat
request with an assistant `tool_calls` entry and a following `tool` role result
that preserve the original call id. Success and model-correctable failure
results MUST both allow the Chat turn to continue.

#### Scenario: read result continues the Chat tool loop

- **WHEN** a mapped `read_file` call completes successfully
- **THEN** the continuation request contains its assistant tool call
- **AND** the following tool message uses the same `tool_call_id` and bounded
  read result

#### Scenario: stale edit failure continues without mutation

- **WHEN** a mapped `edit_file` call fails because its receipt is missing or
  stale
- **THEN** the continuation request contains a model-correctable tool result
- **AND** the target file is unchanged
- **AND** a later read and corrected edit can continue the same turn

### Requirement: Chat history MUST remain stable when legacy tools become hidden

Hiding ApplyPatch in Chat `dedicated` mode MUST affect only current tool
declarations. Codex MUST preserve deterministic names and call ids for prior
assistant tool history, and the hidden handler MUST remain dispatchable for a
valid resumed call.

#### Scenario: prior apply_patch history survives dedicated mode

- **WHEN** conversation history contains an ApplyPatch call and result but the
  current Chat mode is `dedicated`
- **THEN** the assistant history uses ApplyPatch's deterministic Chat name
- **AND** the result references the original call id
- **AND** ApplyPatch is absent from the current request's tool declarations

### Requirement: Chat dedicated file tools MUST be proven with mocked end-to-end turns

Codex MUST include mocked Chat Completions coverage that exercises mapped
declarations, fragmented tool calls, local execution, tool-role continuation,
receipt refresh, stale failure, and remote executor path handling.

#### Scenario: read edit edit completes end to end

- **WHEN** a mocked Chat endpoint drives `read_file`, then `edit_file`, then a
  second `edit_file` across sampling steps
- **THEN** both edits execute through the shared reviewable mutation path
- **AND** the refreshed receipt authorizes the second edit
- **AND** the turn completes with final assistant text

#### Scenario: create then edit completes end to end

- **WHEN** a mocked Chat endpoint drives `write_file` create followed by
  `edit_file`
- **THEN** the create establishes a full receipt
- **AND** the edit succeeds without an intervening read

#### Scenario: remote executor preserves environment-aware paths

- **WHEN** the mocked Chat tool loop runs with an auto-selected remote Linux or
  Windows executor
- **THEN** file paths and receipts use the selected environment contract
- **AND** Chat wire mapping does not reinterpret paths on the core host
