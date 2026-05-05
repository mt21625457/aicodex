## ADDED Requirements

### Requirement: Claude tool declarations must preserve Codex input contracts

Codex MUST expose first-party Codex tools that cross the Claude Messages
adapter boundary with Claude-compatible JSON schemas that preserve the effective
Codex input contract. Required fields, property descriptions, and
`additionalProperties` constraints MUST NOT be dropped when tools are serialized
into Claude `tools` entries.

#### Scenario: Exec command requires cmd under Claude

- **WHEN** Codex serializes `exec_command` for a Claude Messages provider
- **THEN** the Claude tool `input_schema.required` includes `cmd`
- **AND** the `cmd` property description explains that it is the shell command
  to execute
- **AND** malformed `tool_use.input` that omits `cmd` is rejected locally
  without executing a command

#### Scenario: First-party tool schemas survive Claude flattening

- **WHEN** Codex serializes first-party function tools for Claude Messages
- **THEN** each Claude tool entry keeps the same required fields and field
  descriptions as the source Codex tool definition where Claude's schema shape
  supports them
- **AND** namespace flattening does not remove reverse metadata needed to map a
  Claude `tool_use.name` back to the original Codex tool

#### Scenario: Representative shell-like schemas remain aligned

- **WHEN** Codex serializes shell-like tools for Claude Messages
- **THEN** `write_stdin` keeps `session_id` required
- **AND** `shell`, `shell_command`, and `local_shell` keep their command fields
  required with names that match their handlers
- **AND** `request_permissions` keeps `permissions` required

#### Scenario: Tool search keeps its special Claude mapping

- **WHEN** Codex serializes `tool_search` for Claude Messages
- **THEN** the Claude tool `input_schema.required` includes `query`
- **AND** the reverse metadata marks the tool as `ToolSearch`
- **AND** a Claude `tool_use` for `tool_search` maps to a client-executed
  `ToolSearchCall` rather than an ordinary function call

### Requirement: Claude tool inventory must be handler-backed

Codex MUST NOT advertise a Claude Messages tool from a normal turn
`ToolRouter` toolset that cannot be routed to an executable Codex handler path.
Every Claude tool entry from that path MUST have reverse metadata and MUST map
to one of: a registered local handler, a registered MCP handler, a registered
dynamic-tool handler, or an explicit special Claude tool kind such as client
`tool_search`. Claude serialization helpers MUST also defend against hosted
OpenAI tool specs being passed directly by omitting or rejecting those hosted
tools.

#### Scenario: Claude tool entries have reverse mappings

- **WHEN** Codex builds a representative toolset for a Claude provider through
  the normal turn `ToolRouter` path
- **THEN** every serialized Claude `tools[*].name` has exactly one
  `ClaudeToolCallInfo` entry
- **AND** the reverse metadata identifies the original Codex name, namespace,
  and tool-call kind

#### Scenario: Claude tool entries are executable

- **WHEN** Codex builds a representative toolset for a Claude provider
- **THEN** every advertised function or custom tool maps to a registered
  handler for the resulting `ToolPayload`
- **AND** every advertised MCP or dynamic tool maps to the corresponding
  registered handler
- **AND** every advertised special-kind tool maps to the expected special
  routing path

#### Scenario: Unknown Claude tool names remain local errors

- **WHEN** a Claude model invents a tool name that Codex did not advertise
- **THEN** Codex returns a local unsupported-tool result or error
- **AND** this behavior does not justify advertising tools that lack handlers

### Requirement: Claude freeform declarations must preserve grammar-critical input contracts

Codex MUST compensate for Claude Messages lacking OpenAI Responses custom
freeform grammar support when exposing any `ToolSpec::Freeform` tool. A
Claude-facing freeform declaration MUST either preserve the grammar-critical
input instructions in the tool description or `input` property description, or
the tool MUST be omitted or rejected during Claude request construction.

#### Scenario: Freeform tools use explicit JSON wrapper wording

- **WHEN** Codex serializes a freeform tool for Claude Messages
- **THEN** the Claude tool declaration contains a required string field named
  `input`
- **AND** the declaration explains that Claude calls the tool through JSON
  `tool_use.input`
- **AND** the declaration explains that the nested `input` string contains the
  raw freeform body parsed by the handler

#### Scenario: Code Mode exec syntax guidance is present

- **WHEN** Code Mode `exec` is enabled and serialized for Claude Messages
- **THEN** the Claude tool description or `input` property description explains
  that `input` is raw JavaScript source text
- **AND** it states that Markdown code fences and quoted JSON strings are not
  the raw input format
- **AND** it preserves guidance for the optional first-line
  `// @exec: {...}` pragma

#### Scenario: Unsafe freeform conversions are not exposed

- **WHEN** Codex cannot represent a freeform tool's essential grammar or input
  contract safely in Claude's JSON tool shape
- **THEN** Codex omits that tool from Claude serialization or fails request
  construction with an explicit unsupported-tool-contract error
- **AND** it MUST NOT expose the tool with only a generic "raw freeform input"
  description

### Requirement: Claude apply_patch declaration must include the full patch syntax contract

Codex MUST compensate for Claude Messages lacking OpenAI Responses custom
freeform grammar support when exposing `apply_patch`. The Claude-facing
`apply_patch` tool declaration MUST describe the raw patch syntax that the
handler parses, including patch boundaries, hunk headers, and required line
prefixes. The declaration MUST make clear that Claude calls the tool through a
JSON `tool_use.input` object whose `input` string contains the raw patch body.

#### Scenario: Apply patch syntax guidance is present

- **WHEN** Codex serializes `apply_patch` for a Claude Messages provider
- **THEN** the Claude tool description or `input` property description includes
  `*** Begin Patch`
- **AND** it includes `*** Add File: <path>` or equivalent add-file guidance
- **AND** it states that new file content lines must start with `+`
- **AND** it includes an example where `*** Add File:` is followed by a
  `+`-prefixed content line

#### Scenario: Apply patch remains a Codex custom tool internally

- **WHEN** Claude streams a `tool_use` for the Claude-facing `apply_patch` tool
- **THEN** Codex maps it back to the internal `apply_patch` custom/freeform tool
  representation
- **AND** the raw patch body is extracted from the JSON `input` field
- **AND** existing OpenAI Responses freeform `apply_patch` behavior remains
  unchanged

#### Scenario: Misleading freeform wording is not exposed to Claude

- **WHEN** Codex builds the Claude-facing `apply_patch` tool declaration
- **THEN** the declaration MUST NOT instruct Claude to avoid JSON wrapping
  without explaining that only the nested `input` string is raw freeform text
- **AND** the declaration MUST NOT imply that Markdown fences, `Create file:`
  headers, or shell heredocs are the Claude tool input format

### Requirement: OpenAI hosted tools must not be advertised as Claude executable tools

Codex MUST NOT expose OpenAI hosted/server-side tools as callable Claude tools
unless Codex also provides a concrete Claude-compatible execution path for
them. Web search and image generation MUST be absent from Claude provider tool
declarations by default.

#### Scenario: Claude provider toolsets omit hosted tools

- **WHEN** Codex builds tools for a provider whose wire API is Claude
- **THEN** the advertised Claude tool names do not include `web_search`
- **AND** the advertised Claude tool names do not include `image_generation`
- **AND** this remains true even when the general feature flags for web search
  or image generation are enabled

#### Scenario: Hosted tools cannot become dangling Claude functions

- **WHEN** a hosted `web_search` or `image_generation` `ToolSpec` reaches the
  Claude serializer directly
- **THEN** Codex omits it or fails request construction with an explicit
  unsupported-hosted-tool error
- **AND** Codex MUST NOT emit a normal Claude function tool that has no
  executable local handler

### Requirement: Malformed Claude tool input must produce actionable local errors

Codex MUST reject schema-invalid or parser-invalid Claude tool calls before
executing local side effects. The tool result returned to the model MUST include
an actionable error that identifies the violated contract closely enough for the
model to retry with a valid call.

#### Scenario: Missing required field is reported

- **WHEN** Claude calls `exec_command` with `tool_use.input` missing `cmd`
- **THEN** Codex returns a tool-result error mentioning the missing `cmd` field
- **AND** no shell command is executed
- **AND** the turn loop remains able to accept a corrected follow-up tool call

#### Scenario: Missing custom input string is reported

- **WHEN** Claude calls a custom/freeform tool with `tool_use.input` that omits
  the required string field `input`
- **THEN** Codex returns a tool-result error explaining that freeform tools
  require the raw body in `input`
- **AND** Codex MUST NOT stringify the entire JSON object and pass that text to
  the freeform handler

#### Scenario: Non-string custom input is reported

- **WHEN** Claude calls a custom/freeform tool with `tool_use.input.input` set
  to an object, array, number, boolean, or null
- **THEN** Codex returns a tool-result error explaining that `input` must be a
  string
- **AND** the freeform handler is not invoked with a synthesized string

#### Scenario: Invalid apply_patch header is reported

- **WHEN** Claude calls `apply_patch` with a patch body starting with
  `Create file:` instead of `*** Begin Patch`
- **THEN** Codex returns a tool-result error explaining the expected patch
  boundary or hunk header
- **AND** no file is written through the `apply_patch` runtime

#### Scenario: Add-file content without plus prefix is reported

- **WHEN** Claude calls `apply_patch` with `*** Add File: path` followed by
  ordinary file content that does not start with `+`
- **THEN** Codex returns a tool-result error explaining that add-file content
  lines must start with `+`
- **AND** the error does not misclassify the ordinary content line as a valid
  hunk header

#### Scenario: Corrected retry can succeed

- **WHEN** Claude first emits malformed tool input and then retries with input
  that satisfies the advertised schema or patch grammar
- **THEN** Codex executes only the corrected call
- **AND** the final Claude continuation receives the successful tool result

### Requirement: DeepSeek Claude compatibility must be detected by normalized endpoint

Codex MUST detect DeepSeek's Anthropic-compatible Claude endpoint using a
normalized provider base URL rather than relying on provider display names or
model slugs. The detection MUST recognize DeepSeek's supported `/anthropic` and
`/anthropic/v1` URL forms and MUST avoid matching unrelated hosts or paths.

#### Scenario: DeepSeek anthropic v1 endpoint is recognized

- **WHEN** the provider base URL is
  `https://api.deepseek.com/anthropic/v1`
- **THEN** Codex applies DeepSeek Claude compatibility behavior
- **AND** the same behavior applies to trailing-slash variants

#### Scenario: DeepSeek anthropic root endpoint is recognized

- **WHEN** the provider base URL is `https://api.deepseek.com/anthropic`
- **THEN** Codex applies DeepSeek Claude compatibility behavior
- **AND** the same behavior applies to the trailing-slash variant

#### Scenario: Unrelated URLs are not classified as DeepSeek

- **WHEN** the provider base URL host is not `api.deepseek.com`
- **OR** the path is not `/anthropic` or `/anthropic/v1`
- **THEN** Codex MUST NOT classify the provider as DeepSeek based on that URL
  alone

### Requirement: Claude tool-call troubleshooting must distinguish protocol layers

Codex MUST document and test the difference between Claude routing/auth
failures and Claude tool-call contract failures. Once a provider is confirmed to
use `/v1/messages` with Anthropic-compatible auth headers, malformed tool input
MUST be diagnosed as a tool schema or tool parser problem rather than as a
Responses/WebSocket routing problem.

#### Scenario: Manual smoke tests separate routing from tool contracts

- **WHEN** a Claude-compatible provider is manually smoke-tested
- **THEN** the guide first verifies `/v1/messages` routing and Anthropic
  authentication headers
- **AND** tool-call failures such as missing `cmd` or malformed `apply_patch`
  are investigated through Claude tool declarations, local tool-result errors,
  and rollout/diagnostic logs
