## Context

Codex has two different tool declaration surfaces:

- OpenAI Responses can use function tools, namespaces, hosted tools, and custom
  freeform tools with grammar metadata.
- Claude Messages only supports JSON object `tools` entries and streams
  `tool_use` content blocks with JSON input.

The existing Claude adapter correctly maps tool names and streams tool calls
back into Codex `ResponseItem`s, but the current conversion is too lossy for
some first-party tools. `apply_patch` is the clearest example: the OpenAI
freeform tool has a grammar, while the Claude synthetic JSON tool currently only
advertises `input: string` as "Raw freeform tool input." DeepSeek can therefore
select `apply_patch` but still produce patch text that the strict parser rejects.

## Goals / Non-Goals

**Goals:**

- Preserve the effective input contract of Codex first-party tools when they are
  exposed through Claude Messages.
- Make the Claude-facing `apply_patch` declaration self-contained and aligned
  with the parser grammar.
- Make all Claude-exposed freeform tools preserve their essential input
  grammar, including Code Mode `exec` when enabled.
- Prevent OpenAI hosted/server-side tools from being advertised as executable
  Claude tools unless Codex has a concrete handler-backed replacement.
- Prove that every Claude-advertised tool has matching reverse metadata and an
  executable handler path.
- Keep malformed Claude tool input local, actionable, and non-executed.
- Lock DeepSeek Anthropic-compatible base URL detection with explicit positive
  and negative tests.
- Prove the behavior with unit tests and mocked Claude tool-loop tests.

**Non-Goals:**

- Do not route DeepSeek or Claude back through OpenAI Responses/WebSocket.
- Do not add Claude-specific logic to the OpenAI Responses request builder or
  SSE parser.
- Do not make `cat`/heredoc file writes the preferred fallback for malformed
  `apply_patch`.
- Do not relax `apply_patch` syntax to accept arbitrary Markdown or non-Codex
  patch headers.
- Do not implement web search or image generation for Claude in this change.
  The fix is to avoid dangling hosted-tool declarations unless a real
  handler-backed implementation is added separately.
- Do not introduce a broad provider-specific transcript model.

## Decisions

### 1. Claude tool inventory must be audited by category

The issue is not that every Claude tool is broken. The current risk depends on
how a `ToolSpec` is converted and how the resulting `tool_use` is routed:

| Tool category | Current Claude behavior | Risk | Required hardening |
| --- | --- | --- | --- |
| Function tools | Preserve `parameters` as `input_schema` and route as `FunctionCall` | Mostly OK, but regressions can drop required fields or descriptions | Schema preservation tests for representative first-party tools |
| Namespace/MCP tools | Flatten namespace and function name, preserve schema, keep reverse metadata | Mostly OK, but name collision or metadata loss would misroute calls | Reverse metadata and handler-backed inventory tests |
| Dynamic tools | Convert to function or namespace specs, then use the same path above | Mostly OK, but schemas must survive dynamic conversion and Claude flattening | Include dynamic tools in schema and handler parity tests |
| Tool search | Preserve schema and map back as `ToolSearchCall` with `execution = "client"` | Special path can regress independently of function tools | Test query schema and stream mapping |
| Local shell / shell-like tools | Use function schemas or synthetic `local_shell` schema and shell handlers | Field-name drift can produce missing-field errors or incompatible payloads | Required-field and handler conformance tests for shell variants |
| Freeform/custom tools | Convert to JSON `{ input: string }` and map back as `CustomToolCall` | High risk: grammar metadata is lost; malformed `input` may be stringified | Preserve grammar-critical instructions or omit the tool |
| OpenAI hosted tools | `image_generation` is skipped; `web_search` has a synthetic function mapping | High risk if exposed: no local handler-backed Claude implementation | Do not advertise under Claude unless backed by a handler |

The implementation should encode this inventory as tests rather than relying on
tribal knowledge. Handler-backed inventory checks should use a representative
Claude toolset built through the same `ToolRouter`/provider capability path used
by turns, because the low-level serializer does not own the handler registry.
Every serialized Claude tool from that normal turn path should be checked for:

- a `ClaudeToolCallInfo` reverse mapping;
- a tool kind that the Claude stream parser maps intentionally;
- a `ToolRegistry` handler or special handler path for the resulting
  `ToolPayload`; and
- no OpenAI hosted tools unless an explicit Claude implementation exists.

### 2. Claude tool declarations must preserve the real Codex contract

`codex-tools` should remain the owner of tool declaration serialization. For
Claude, that means first-party function tools exposed through this adapter must
keep their JSON schema, required fields, and field descriptions when flattened
into a Claude `tools` entry. This proposal should focus regression coverage on
tools that already cross the Claude boundary, starting with `exec_command` and
covering representative shell-like, permission, plan, image-view, agent,
request-user-input, tool-search, MCP, and dynamic-tool specs.

Tests should compare the Claude tool JSON for first-party tools that have caused
production issues. At minimum:

- `exec_command` includes an `input_schema.required` entry for `cmd`;
- `exec_command.properties.cmd.description` remains visible;
- `write_stdin` keeps `session_id` required;
- `shell`, `shell_command`, and synthetic `local_shell` keep their respective
  command fields required and aligned with their handlers;
- `request_permissions` keeps `permissions` required;
- `tool_search` keeps `query` required and reverse metadata kind `ToolSearch`;
- additional first-party tool fields are not accidentally dropped during Claude
  flattening.

### 3. Freeform tools need Claude-specific synthetic JSON contracts

Claude cannot receive the OpenAI custom/freeform grammar directly. Codex should
therefore treat all `ToolSpec::Freeform` tools as grammar-sensitive when
converting to Claude. The generic fallback description `"Raw freeform tool
input."` is not sufficient because it says nothing about the syntax enforced by
the handler.

For every Claude-exposed freeform tool, Codex must choose one of these outcomes:

- expose a Claude JSON schema with a required string `input` field and
  description text that preserves the tool's essential grammar and examples; or
- omit/fail fast if the freeform contract cannot be represented safely for
  Claude.

This applies immediately to:

- `apply_patch`, whose raw `input` string must be the Codex patch body; and
- Code Mode `exec`, whose raw `input` string must be JavaScript source text and
  may start with the supported `// @exec: {...}` pragma.

The Claude stream parser should not silently stringify a malformed custom tool
input object when the declared freeform tool requires `input: string`. Missing
or non-string `input` should become a local tool-result error that tells the
model to place the raw freeform body in the `input` string.

### 4. `apply_patch` needs a complete patch contract

`apply_patch` needs the most explicit freeform conversion because its parser is
strict and the observed failures came from patch syntax drift. Codex should
treat `apply_patch` as a special freeform-to-Claude conversion:

- keep the Claude tool name as `apply_patch`;
- keep reverse metadata as `ClaudeToolCallKind::Custom` so the rest of Codex can
  continue receiving a `CustomToolCall` with the extracted patch body;
- expose a Claude JSON schema with a required `input` string;
- set the tool description and/or `input` property description to the complete
  patch grammar instructions, including an add-file example with `+` prefixes.

The Claude-facing text must not say "do not wrap the patch in JSON" without
context, because Claude must wrap the string in a JSON `tool_use.input` object.
The precise rule is: the `input` string itself is raw patch text; the Claude
tool call is JSON.

The implementation may use a concise grammar-focused description rather than
copying every general Codex editing instruction. The minimum contract is the
patch envelope, supported file operation headers, add-file `+` prefixes, update
line prefixes, and a complete valid example.

### 5. OpenAI hosted tools must not become dangling Claude tools

Claude providers currently disable `web_search` and `image_generation` through
provider capabilities, and that should remain the primary filter for real turns.
The serializer should still be robust if a hosted `ToolSpec` reaches
`create_tools_json_for_claude_messages` directly.

The acceptable behaviors are:

- omit hosted tools from Claude serialization when no Claude-local handler
  exists; or
- return an explicit build-time error explaining that the hosted tool is not
  Claude executable.

What should not happen is a Claude tool entry named `web_search` or
`image_generation` that routes as a normal function call but has no registered
handler. Such entries create avoidable tool-loop failures that look like model
mistakes even though the adapter advertised an impossible tool.

### 6. Malformed tool inputs become local tool-result errors

For Claude tool calls that map to Codex first-party tools, schema-invalid input
must not execute. Codex should return a tool-result error that includes enough
information for the model to retry correctly, for example:

- missing required field `cmd` for `exec_command`;
- invalid `apply_patch` header;
- `*** Add File:` content lines must start with `+`; or
- missing or non-string `input` for custom/freeform tools.

This behavior should reuse existing tool handler validation wherever possible.
If parser messages are too generic, improve the specific parser error rather
than adding protocol-specific string matching in the Claude stream parser. Parser
diagnostic changes belong in `codex-apply-patch`; Claude-specific tool-loop
tests belong in `codex-core`.

### 7. DeepSeek compatibility detection should normalize endpoint URLs

DeepSeek compatibility should be detected from normalized provider base URLs.
The URL logic should parse the host and path rather than matching arbitrary
substrings or depending on provider display names or model slugs.

Valid DeepSeek Claude-compatible examples:

- `https://api.deepseek.com/anthropic`
- `https://api.deepseek.com/anthropic/`
- `https://api.deepseek.com/anthropic/v1`
- `https://api.deepseek.com/anthropic/v1/`

Invalid examples:

- `https://notapi.deepseek.com/anthropic`
- `https://api.deepseek.com/other`
- unrelated Anthropic-compatible provider URLs that merely contain the word
  `deepseek` in a query parameter.

If the implementation branch already recognizes all valid URL forms, the work
for this decision is regression coverage and documentation, not a second
normalization rewrite.

### 8. Diagnostics should point at the tool contract, not provider routing

Once `wire_api = "claude"` sends requests to `/v1/messages` with Anthropic auth
headers, diagnostics for failures in this class should identify malformed tool
input. Logs and manual test docs should help distinguish:

- routing/auth failures;
- Claude stream parsing failures;
- schema-invalid tool calls; and
- parser-invalid `apply_patch` bodies.

## Verification

- `cd codex-rs && just fmt`
- `cd codex-rs && cargo test -p codex-tools claude`
- `cd codex-rs && cargo test -p codex-code-mode` if Code Mode wording or
  diagnostics change
- `cd codex-rs && cargo test -p codex-apply-patch` if parser diagnostics change
- `cd codex-rs && cargo test -p codex-model-provider` if provider capability
  filtering changes
- `cd codex-rs && cargo test -p codex-core claude`
- `cd codex-rs && cargo test -p codex-api claude` if stream diagnostics change
- `cd codex-rs && just fix -p codex-tools`
- `cd codex-rs && just fix -p codex-code-mode` if Code Mode changes
- `cd codex-rs && just fix -p codex-apply-patch` if parser diagnostics change
- `cd codex-rs && just fix -p codex-model-provider` if provider changes
- `cd codex-rs && just fix -p codex-core`
- `openspec validate harden-claude-tool-call-contracts --strict`

If config schema changes, also run:

- `cd codex-rs && just write-config-schema`

If Rust dependencies change, also run from the repository root:

- `just bazel-lock-update`
- `just bazel-lock-check`
