# Configuration

For basic configuration instructions, see [this documentation](https://developers.openai.com/codex/config-basic).

For advanced configuration instructions, see [this documentation](https://developers.openai.com/codex/config-advanced).

For a full configuration reference, see [this documentation](https://developers.openai.com/codex/config-reference).

## Lifecycle hooks

Admins can set top-level `allow_managed_hooks_only = true` in
`requirements.toml` to ignore user, project, and session hook configs while
still allowing managed hooks from requirements and managed config layers. This
setting is only supported in `requirements.toml`; putting it in `config.toml`
does not enable managed-hooks-only mode.

## Claude native protocol policy

Claude Messages providers use a capability-gated tool plan. Anthropic-native
`text_editor` is emitted only when the existing `apply_patch` edit path is
available, and mutations are translated through Codex file scoping, approvals,
and patch reporting. DeepSeek-compatible Claude providers stay on the
user-defined/freeform fallback surface, so `apply_patch` remains advertised
instead of Anthropic-only native tool types.

Native `bash` calls are executed through the existing Codex shell approval,
sandbox, timeout, cwd/env, stdout/stderr, and exit-code path. Anthropic server
`code_execution` is a separate provider-side environment from local shell; do
not assume files, variables, cwd, or process state are shared between them.

Remote Anthropic MCP connector support is modeled as provider-side
`mcp_servers` plus `mcp_toolset`, requires HTTPS remote servers and the
`mcp-client-2025-11-20` beta header, and remains opt-in because it sends tool
traffic through the provider. Local STDIO/private MCP servers continue to use
Codex-managed flattened tool names and local execution.

Claude server-tool result blocks and citation metadata are preserved for
follow-up turns. Clients that do not understand structured citations still
receive visible source markers as a compatibility fallback.
