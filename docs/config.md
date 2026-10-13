# Configuration

For basic configuration instructions, see [this documentation](https://developers.openai.com/codex/config-basic).

For advanced configuration instructions, see [this documentation](https://developers.openai.com/codex/config-advanced).

For a full configuration reference, see [this documentation](https://developers.openai.com/codex/config-reference).

## Model-provider wire protocol policy

The internal `model_providers.<id>.wire_api` selector controls only the provider wire adapter:

- omitted or `"responses"` uses OpenAI Responses and remains the default;
- `"claude"` (alias `"anthropic"`) uses Anthropic Messages;
- `"chat"` uses OpenAI-compatible Chat Completions at `chat/completions` relative to the
  configured provider base URL.

All model-provider HTTP requests and WebSocket handshakes, including Responses and Realtime
transports, use the exact versioned product token `aicodex/<workspace package version>` as
`User-Agent`, independent of the selected wire protocol. Host-protocol identity returned by
app-server or MCP initialization is a separate compatibility surface.

## DeepSeek Responses image history

DeepSeek Responses requests exceeding 80% of their effective uncompressed request
byte budget replace the oldest inline tool-result images with explicit text notices
until the request reaches that target or no eligible images remain. This applies to
`deepseek-*` model names, including provider-prefixed names; other models sharing
the same provider are unaffected.

User attachments, standalone host-delivered inputs without a call ID, and the latest
image-bearing tool batch (including parallel results) are retained. Images using a
file ID or remote URL also establish the latest batch but are never omission
candidates. Tool arguments, file/page references, surrounding text, and existing
conclusions remain in place so the model can reload an older image when
needed. Only the outgoing request copy changes; local images and durable history
are preserved, including when switching back to another model.

The effective budget still uses `model_providers.<id>.request_body_max_bytes` and
the existing DeepSeek ceiling of 40 MiB. The 80% target is best effort: protected
images may exceed it. Existing image encoding optimization and the hard request
limit still apply, so a request that cannot fit fails before it is sent.

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
