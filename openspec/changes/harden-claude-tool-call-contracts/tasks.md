## 1. Scope and Reproduction

- [x] 1.1 Capture the known-good Claude routing baseline: provider uses
  `wire_api = "claude"`, `/v1/messages`, `x-api-key`, and
  `anthropic-version`.
- [x] 1.2 Add or update a focused reproduction for a Claude `exec_command`
  `tool_use` missing the required `cmd` field.
- [x] 1.3 Add or update focused reproductions for malformed Claude
  `apply_patch` calls: non-Codex hunk header and add-file content without `+`
  prefixes.
- [x] 1.4 Add a focused reproduction for a malformed Claude custom/freeform
  tool call whose `tool_use.input` omits `input` or sets it to a non-string.

## 2. Claude Tool Inventory and Handler Parity

- [x] 2.1 Audit every `ToolSpec` variant in
  `create_tools_json_for_claude_messages`: `Function`, `Namespace`,
  `ToolSearch`, `LocalShell`, `Freeform`, `WebSearch`, and
  `ImageGeneration`.
- [x] 2.2 Add tests that build a representative Claude provider toolset through
  the normal `ToolRouter`/provider capability path.
- [x] 2.3 Assert every directly serialized Claude `tools` entry has exactly one
  `ClaudeToolCallInfo` reverse mapping.
- [x] 2.4 Assert every advertised Claude tool maps to an executable local, MCP,
  dynamic, or special-kind handler path.
- [x] 2.5 Assert real Claude provider prompts do not advertise `web_search` or
  `image_generation` unless a future handler-backed Claude implementation is
  explicitly added.
- [x] 2.6 Make direct Claude serialization of OpenAI hosted tools either omit
  them or fail fast with a clear unsupported-hosted-tool error; do not leave a
  dangling `web_search` function declaration.

## 3. Claude Tool Declaration Contract

- [x] 3.1 Audit first-party tool schema preservation for tools that cross the
  Claude adapter boundary.
- [x] 3.2 Add `codex-tools` tests proving `exec_command` keeps `cmd` in
  `input_schema.required` under Claude serialization.
- [x] 3.3 Add representative required-field tests for `write_stdin`,
  `shell`, `shell_command`, `local_shell`, `request_permissions`, and
  `tool_search`.
- [x] 3.4 Add tests proving relevant first-party tool property descriptions
  survive Claude serialization.
- [x] 3.5 Add MCP namespace and dynamic-tool tests proving input schemas survive
  Claude flattening and reverse metadata remains correct.
- [x] 3.6 Keep namespace flattening and reverse tool-call metadata unchanged.

## 4. Claude Freeform Tool Contract

- [x] 4.1 Replace the generic Claude freeform `input` description with a
  grammar-aware description strategy.
- [x] 4.2 For every Claude-exposed freeform tool, either preserve essential
  grammar/input instructions in the Claude declaration or omit/fail fast if the
  contract cannot be safely represented.
- [x] 4.3 Add a shared or Claude-specific apply-patch description that includes
  the complete patch grammar and examples.
- [x] 4.4 Use that description when converting `ToolSpec::Freeform` with
  `name = "apply_patch"` into a Claude JSON tool.
- [x] 4.5 Update the Claude-facing `apply_patch.input` property description so
  it explains that the `input` string is the entire raw patch body.
- [x] 4.6 Ensure the Claude-facing `apply_patch` description contains
  `*** Begin Patch`, `*** Add File`, and a `+`-prefixed add-file content
  example.
- [x] 4.7 Preserve Code Mode `exec` input guidance under Claude, including raw
  JavaScript source text and the optional `// @exec: {...}` pragma.
- [x] 4.8 Avoid Claude-facing wording that implies Claude should call freeform
  tools outside its JSON `tool_use.input` object.
- [x] 4.9 Add tests proving the final Claude tool JSON contains the required
  freeform syntax guidance for both `apply_patch` and Code Mode `exec` when
  enabled.

## 5. Malformed Tool Input Handling

- [x] 5.1 Verify schema-invalid Claude function tool calls are returned as
  tool-result errors and are not executed.
- [x] 5.2 Reject malformed Claude custom/freeform calls with missing or
  non-string `input` as local tool-result errors; do not silently stringify the
  entire object as raw tool input.
- [x] 5.3 Improve `apply_patch` parser errors if needed so add-file content
  without `+` reports the missing prefix directly.
- [x] 5.4 Add mocked Claude tool-loop tests for malformed `exec_command` and
  `apply_patch` inputs followed by corrected successful tool calls where
  applicable.
- [x] 5.5 Add mocked Claude tool-loop tests for malformed Code Mode `exec`
  inputs if Code Mode freeform execution is enabled in a Claude provider turn.
- [x] 5.6 Ensure diagnostics distinguish malformed tool input from provider
  routing or authentication errors.

## 6. DeepSeek Claude Compatibility

- [x] 6.1 Verify or implement normalized provider base URL handling before
  DeepSeek compatibility checks.
- [x] 6.2 Ensure tests recognize `/anthropic`, `/anthropic/`,
  `/anthropic/v1`, and `/anthropic/v1/` on `api.deepseek.com`.
- [x] 6.3 Add negative tests for unrelated hosts and unrelated paths.
- [x] 6.4 Do not rely on provider display names or model slugs as the primary
  DeepSeek compatibility signal.

## 7. Documentation and Manual Testing

- [x] 7.1 Update `test/claude-wire-api/README.md` with a troubleshooting split
  between routing/auth failures and tool-call contract failures.
- [x] 7.2 Confirm `docs/config.md` does not need an update because the DeepSeek
  Claude provider guidance did not change.
- [x] 7.3 Document the `apply_patch` Claude rule: JSON tool call wrapper,
  raw patch body inside the `input` string.
- [x] 7.4 Document the general Claude freeform rule for `apply_patch` and Code
  Mode `exec`: Claude calls JSON, the nested `input` string contains the raw
  freeform body.
- [x] 7.5 Document that OpenAI hosted web search and image generation are not
  Claude-executable tools unless a future handler-backed Claude implementation
  is added.

## 8. Verification

- [x] 8.1 Run `cd codex-rs && just fmt`.
- [x] 8.2 Run `cd codex-rs && cargo test -p codex-tools claude`.
- [x] 8.3 Skip `cd codex-rs && cargo test -p codex-code-mode` because Code Mode
  crate behavior did not change; Claude-facing `exec` wording is covered by
  `codex-tools` tests.
- [x] 8.4 Run `cd codex-rs && cargo test -p codex-apply-patch` because parser
  diagnostics changed.
- [x] 8.5 Skip `cd codex-rs && cargo test -p codex-model-provider` because
  provider capability filtering did not change.
- [x] 8.6 Run `cd codex-rs && cargo test -p codex-core --test all
  suite::claude_wire` and targeted core unit tests. `cargo test -p codex-core
  claude` also ran, but two pre-existing client auth tests failed outside this
  change path.
- [x] 8.7 Run `cd codex-rs && cargo test -p codex-api claude` because stream or
  endpoint diagnostics change.
- [x] 8.8 Run scoped `just fix -p <crate>` for changed Rust crates after tests
  pass.
- [x] 8.9 Run `openspec validate harden-claude-tool-call-contracts --strict`.
