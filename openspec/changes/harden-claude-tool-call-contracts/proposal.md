## Why

The Claude Messages provider path is now wired far enough that request routing,
authentication, and endpoint shape are not the primary failure mode. The current
DeepSeek/Claude configuration uses `wire_api = "claude"`, disables Responses
websockets, and sends Claude requests through `/v1/messages` with Anthropic-style
headers.

Recent failures are instead happening at the Codex tool-call contract boundary:

- DeepSeek emitted an `exec_command` tool call without the required `cmd` field,
  even though the Codex schema requires it.
- DeepSeek repeatedly emitted malformed `apply_patch` input: first using a
  non-Codex `Create file:` header and then omitting `+` prefixes required for
  `*** Add File:` content lines.
- OpenAI Responses can expose `apply_patch` as a custom/freeform tool with a
  grammar, but Claude Messages cannot. Codex currently converts
  `ToolSpec::Freeform` into a regular Claude JSON tool with only an
  `input: string` field and a short "raw freeform input" description. That loses
  the full patch grammar that the strict `apply_patch` handler enforces.
- The same freeform-to-JSON downgrade applies to any other Codex freeform tool
  exposed through Claude, including Code Mode `exec` when enabled. Those tools
  need an explicit "JSON wrapper, raw string input" contract and must reject
  missing or non-string `input` values instead of silently stringifying the
  entire JSON object.
- OpenAI hosted/server-side tools are not Claude-native. The Claude provider
  capability path currently disables web search and image generation, but the
  Claude serializer still contains a synthetic `web_search` mapping. That
  creates a dangling tool risk if a hosted tool bypasses provider capability
  filtering.
- DeepSeek compatibility must be locked to normalized endpoint detection so
  custom provider names or model aliases do not change behavior. Branches that
  already recognize `/anthropic/v1` still need regression coverage and manual
  guidance for that contract.

The result is a loop where the provider can call the right tool name but produce
tool inputs that Codex rejects locally. This proposal hardens the Claude tool
declaration and validation contract without reverting to OpenAI Responses or
moving Claude behavior into the Responses parser.

## What Changes

- Strengthen Claude tool declarations for Codex first-party tools:
  - preserve JSON schema `required` fields and property descriptions for Claude
    tool entries;
  - add conformance tests that prove `exec_command` still advertises `cmd` as
    required under Claude;
  - add representative schema conformance tests for shell-like, stdin,
    permission, plan, image-view, agent, request-user-input, tool-search, MCP,
    and dynamic-tool categories that cross the Claude adapter;
  - ensure every Claude-advertised tool has reverse metadata and an executable
    local, MCP, dynamic, or special-kind handler;
  - return clear local tool-result errors for malformed Claude tool inputs
    without executing partial or schema-invalid commands.
- Give Claude providers a correct `apply_patch` contract:
  - when converting the freeform `apply_patch` tool to a Claude JSON tool,
    include the full patch syntax guidance and examples;
  - make the Claude-facing `input` property describe the entire patch body,
    including `*** Begin Patch`, `*** Add File`, and `+`-prefixed added lines;
  - remove or avoid misleading Claude-facing text that says the tool should not
    be wrapped in JSON, since Claude must call it through a JSON `tool_use`
    object.
- Generalize the freeform-tool contract for Claude:
  - `ToolSpec::Freeform` conversions must preserve essential grammar/input
    instructions in the Claude tool description or `input` property
    description;
  - Code Mode `exec` must describe raw JavaScript source input and its optional
    `// @exec: {...}` first-line pragma when exposed through Claude;
  - malformed custom/freeform Claude calls with missing or non-string `input`
    must produce actionable local errors.
- Keep OpenAI hosted tools out of Claude executable tool declarations unless
  Codex has a real handler-backed replacement:
  - assert actual Claude provider prompts do not advertise `web_search` or
    `image_generation`;
  - make `create_tools_json_for_claude_messages` avoid producing dangling
    `web_search` entries if hosted tools are passed directly to the serializer,
    or fail fast with an explicit unsupported-hosted-tool error.
- Add Claude/DeepSeek mock coverage for the observed failures:
  - malformed `exec_command` input with missing `cmd`;
  - malformed `apply_patch` headers;
  - malformed `*** Add File:` content without `+` prefixes;
  - a corrected follow-up tool call that succeeds after an actionable local
    error.
- Lock DeepSeek Claude compatibility detection:
  - recognize `https://api.deepseek.com/anthropic`,
    `https://api.deepseek.com/anthropic/`,
    `https://api.deepseek.com/anthropic/v1`, and trailing-slash variants;
  - use normalized endpoint checks rather than display-name or model-slug
    heuristics for the primary compatibility decision.
- Update Claude manual test guidance so future debugging focuses on tool-call
  semantics once routing and auth are known-good.

## Capabilities

### New Capabilities

- `claude-tool-call-contract-hardening`: Codex can expose first-party tools to
  Claude-compatible providers with enough schema and prose guidance for those
  providers to construct valid tool inputs.
- `claude-tool-inventory-parity`: Codex can prove that every tool advertised to
  Claude is backed by the expected local, MCP, dynamic, or special Claude tool
  handler path, and that OpenAI hosted tools are not accidentally exposed as
  callable Claude tools.
- `deepseek-claude-compatibility-detection`: Codex can detect DeepSeek's
  Anthropic-compatible endpoint by normalized base URL, including `/anthropic/v1`.

### Modified Capabilities

- `claude-wire-api-support`: Claude Messages routing gains stricter tool
  declaration conformance, `apply_patch` grammar preservation, malformed
  tool-input handling, and DeepSeek endpoint compatibility tests.

## Impact

- Affected crates:
  - `codex-rs/tools`
  - `codex-rs/code-mode` if Code Mode `exec` wording or diagnostics change
  - `codex-rs/apply-patch` if parser diagnostics are improved
  - `codex-rs/model-provider` if Claude provider capability tests or hosted
    tool filtering move closer to provider capability code
  - `codex-rs/core`
  - `codex-rs/codex-api` only if malformed Claude stream/tool diagnostics need
    parser-level changes
- Affected docs:
  - `test/claude-wire-api/README.md`
  - `docs/config.md` only if user-facing Claude/DeepSeek configuration guidance
    changes
- Compatibility:
  - OpenAI Responses request construction and custom/freeform tool behavior stay
    unchanged.
  - Claude providers continue to receive Anthropic Messages `tools` entries and
    `tool_use` blocks.
  - `apply_patch` remains internally represented as a Codex custom/freeform tool
    where needed; only the Claude-facing declaration becomes more explicit.
  - Code Mode `exec` remains internally represented as a Codex custom/freeform
    tool where needed; only the Claude-facing declaration and validation become
    explicit.
  - Claude providers continue to suppress OpenAI hosted web search and image
    generation unless a future handler-backed Claude implementation is added.
- Primary risks:
  - Longer Claude tool descriptions increase prompt/tool declaration tokens.
    This is acceptable for `apply_patch` because malformed patches waste entire
    tool-loop turns and can lead to unsafe shell fallbacks.
  - Returning richer local errors may cause some models to retry more often.
    Tests should prove retries remain bounded by the existing turn/tool loop.
  - DeepSeek URL normalization must be precise enough not to classify unrelated
    Anthropic-compatible providers as DeepSeek.
- Rollback:
  - Revert the Claude-specific `apply_patch` description expansion while keeping
    schema preservation tests.
  - Revert DeepSeek URL normalization independently if it misclassifies a
    provider.
