# Configuration

For basic configuration instructions, see [this documentation](https://developers.openai.com/codex/config-basic).

For advanced configuration instructions, see [this documentation](https://developers.openai.com/codex/config-advanced).

For a full configuration reference, see [this documentation](https://developers.openai.com/codex/config-reference).
## Connecting to MCP servers

AICodex can connect to MCP servers configured in `~/.aicodex/config.toml`. See the configuration reference for the latest MCP server options:

- https://developers.openai.com/codex/config-reference

MCP tools default to serialized calls. To mark every tool exposed by one server
as eligible for parallel tool calls, set `supports_parallel_tool_calls` on that
server:

```toml
[mcp_servers.docs]
command = "docs-server"
supports_parallel_tool_calls = true
```

Only enable parallel calls for MCP servers whose tools are safe to run at the
same time. If tools read and write shared state, files, databases, or external
resources, review those read/write race conditions before enabling this setting.

## Anthropic Claude

Codex can use Anthropic's native Claude Messages API through the `claude` wire
protocol:

```toml
model_provider = "anthropic"
model = "claude-sonnet-4-5"

[model_providers.anthropic]
name = "Anthropic"
base_url = "https://api.anthropic.com/v1"
env_key = "ANTHROPIC_API_KEY"
wire_api = "claude"
```

The `claude` protocol posts to `/v1/messages` and uses `x-api-key` plus
`anthropic-version: 2023-06-01`. The `anthropic` wire_api value is accepted as a
backward-compatible alias for `claude`.

Claude requests preserve base64 data URL images and HTTP(S) image URLs as
native Anthropic image blocks. Structured tool outputs that contain text and
images are sent back as Claude `tool_result` content blocks when possible.
Claude providers do not expose OpenAI server-side tools such as Responses web
search or image generation; local/function-style Codex tools remain available
through Claude `tool_use` blocks.

If a turn provides an output JSON schema, Claude requests include an instruction
to answer with matching JSON. This is not the same as OpenAI Responses
server-side strict JSON-schema validation: `output_schema_strict` remains a
prompt-guided constraint for Claude providers unless a future Claude-specific
enforcement strategy is added.

When `model_reasoning_effort` is set for a Claude provider, Codex sends
Anthropic extended thinking using a budget mapped from the selected effort.
`service_tier = "fast"` maps to Anthropic `service_tier = "auto"`, while
`service_tier = "flex"` maps to `service_tier = "standard_only"`.

DeepSeek's Anthropic-compatible endpoint uses the same `claude` wire protocol
with `base_url = "https://api.deepseek.com/anthropic"` or
`base_url = "https://api.deepseek.com/anthropic/v1"`. For this official
DeepSeek endpoint, Codex also sends DeepSeek's `output_config.effort` when
`model_reasoning_effort` is set: `minimal`, `low`, `medium`, and `high` map to
`"high"`, while `xhigh` maps to `"max"`. Setting
`model_reasoning_effort = "none"` sends an explicit disabled thinking toggle.

The Claude adapter also has protocol support for Anthropic prompt-cache markers
at supported stable-prefix locations, including tool definitions, system text
blocks, and prior message content blocks. This is currently controlled by the
Claude request policy inside Codex rather than a public `config.toml` setting,
so normal Claude requests keep the same shape unless that policy is explicitly
enabled by code or tests.

Codex exposes a Claude endpoint client for `POST /v1/messages/count_tokens`.
The count request reuses the typed Claude message, system, tool, thinking, and
service-tier request model while omitting streaming-only fields. After a
successful Claude streaming turn, Codex refreshes current context-window usage
with `count_tokens`; if the endpoint is unavailable, rejected, rate-limited, or
times out, Codex falls back to the local context estimate. Completion usage from
the stream remains available for response-level accounting when the provider
returns it.

For long-running Claude responses, Codex preserves Claude `pause_turn` stop
reasons separately from client-side `tool_use`. A `pause_turn` response is
continued by sending the assistant content already emitted by Claude in the next
`/v1/messages` request, with an automatic continuation cap to avoid loops.
Claude provider-state blocks such as `compaction` are stored as opaque
provider-state history and re-emitted to Claude on the follow-up request.
Unsupported user-visible Claude blocks are rendered as explicit placeholders
instead of being silently dropped.

## MCP tool approvals

Codex stores approval defaults and per-tool overrides for custom MCP servers
under `mcp_servers` in `~/.aicodex/config.toml`. Set
`default_tools_approval_mode` on the server to apply a default to every tool,
and use per-tool `approval_mode` entries for exceptions:

```toml
[mcp_servers.docs]
command = "docs-server"
default_tools_approval_mode = "approve"

[mcp_servers.docs.tools.search]
approval_mode = "prompt"
```

## Apps (Connectors)

Use `$` in the composer to insert a ChatGPT connector; the popover lists accessible
apps. The `/apps` command lists available and installed apps. Connected apps appear first
and are labeled as connected; others are marked as can be installed.

Codex stores "never show again" choices for tool suggestions in `config.toml`:

```toml
[tool_suggest]
disabled_tools = [
  { type = "plugin", id = "slack@openai-curated" },
  { type = "connector", id = "connector_google_calendar" },
]
```

## Notify

`notify` is deprecated and will be removed in a future release. Existing configurations still work for compatibility, but new automation should use lifecycle hooks instead.

Codex can run a legacy notification command when the agent finishes a turn. See the configuration reference for the latest notification settings:

- https://developers.openai.com/codex/config-reference

When Codex knows which client started the turn, the legacy notify JSON payload also includes a top-level `client` field. The TUI reports `codex-tui`, and the app server reports the `clientInfo.name` value from `initialize`.

## JSON Schema

The generated JSON Schema for `config.toml` lives at `codex-rs/core/config.schema.json`.

## SQLite State DB

Codex stores the SQLite-backed state DB under `sqlite_home` (config key) or the
`CODEX_SQLITE_HOME` environment variable. When unset, WorkspaceWrite sandbox
sessions default to a temp directory; other modes default to `AICODEX_HOME`,
with `CODEX_HOME` honored as a compatibility fallback.

## Custom CA Certificates

Codex can trust a custom root CA bundle for outbound HTTPS and secure websocket
connections when enterprise proxies or gateways intercept TLS. This applies to
login flows and to Codex's other external connections, including Codex
components that build reqwest clients or secure websocket clients through the
shared `codex-client` CA-loading path and remote MCP connections that use it.

Set `CODEX_CA_CERTIFICATE` to the path of a PEM file containing one or more
certificate blocks to use a Codex-specific CA bundle. If
`CODEX_CA_CERTIFICATE` is unset, Codex falls back to `SSL_CERT_FILE`. If
neither variable is set, Codex uses the system root certificates.

`CODEX_CA_CERTIFICATE` takes precedence over `SSL_CERT_FILE`. Empty values are
treated as unset.

The PEM file may contain multiple certificates. Codex also tolerates OpenSSL
`TRUSTED CERTIFICATE` labels and ignores well-formed `X509 CRL` sections in the
same bundle. If the file is empty, unreadable, or malformed, the affected Codex
HTTP or secure websocket connection reports a user-facing error that points
back to these environment variables.

## Notices

Codex stores "do not show again" flags for some UI prompts under the `[notice]` table.

## Plan mode defaults

`plan_mode_reasoning_effort` lets you set a Plan-mode-specific default reasoning
effort override. When unset, Plan mode uses the built-in Plan preset default
(currently `medium`). When explicitly set (including `none`), it overrides the
Plan preset. The string value `none` means "no reasoning" (an explicit Plan
override), not "inherit the global default". There is currently no separate
config value for "follow the global default in Plan mode".

## Realtime start instructions

`experimental_realtime_start_instructions` lets you replace the built-in
developer message Codex inserts when realtime becomes active. It only affects
the realtime start message in prompt history and does not change websocket
backend prompt settings or the realtime end/inactive message.

Ctrl+C/Ctrl+D quitting uses a ~1 second double-press hint (`ctrl + c again to quit`).
