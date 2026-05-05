# Claude Wire API Manual Test

This directory is an isolated `CODEX_HOME` for manual Claude Messages API
testing. It does not use or modify `~/.codex/config.toml`.

## Fill in credentials

1. Edit `config.toml` and set:
   - `model`
   - `model_providers.anthropic.base_url`

2. Copy `.env.example` to `.env` and set:
   - `ANTHROPIC_API_KEY`

The `.env` file is ignored by git.

## Run a smoke test

From the repository root:

```bash
bash test/claude-wire-api/run-smoke.sh
```

The script sets `CODEX_HOME` to this directory and runs `codex exec` with the
Claude provider from `config.toml`.

This smoke path exercises the normal `/v1/messages` stream. It should not call
`/v1/messages/count_tokens`; token counting is available through the Claude
endpoint client for preflight callers, not as an unconditional extra request on
ordinary turns.

When debugging with an HTTP proxy or a local Anthropic-compatible mock, inspect
these Claude-specific boundaries:

- requests include `anthropic-version: 2023-06-01` and `x-api-key`;
- prompt-cache markers, when enabled by Codex's internal Claude request policy,
  appear only on supported stable-prefix blocks such as tools, system text
  blocks, or prior message content;
- `pause_turn` responses are followed by another `/v1/messages` request that
  includes the assistant content from the paused response;
- provider-state blocks such as `compaction` are preserved and re-emitted on the
  follow-up Claude request;
- unsupported user-visible Claude blocks should produce explicit placeholder
  text rather than disappearing from the assistant response.

## Troubleshoot tool calls

Once requests are reaching `/v1/messages` with `anthropic-version` and
`x-api-key`, treat failures such as `missing field cmd` or malformed
`apply_patch` bodies as tool-call contract failures, not Responses/WebSocket
routing failures.

Claude tools are always called through JSON `tool_use.input`. For Codex
freeform tools, the nested `input` string contains the raw freeform body:

- `apply_patch`: `tool_use.input` is `{"input":"*** Begin Patch\n..."}`. The
  string itself must use Codex patch syntax, including `*** Add File: <path>`
  and `+` prefixes for every new-file content line.
- Code Mode `exec`: `tool_use.input` is `{"input":"// @exec: {...}\n..."}` or
  raw JavaScript source text. Do not send Markdown fences as the raw input.

If a Claude-compatible provider omits the nested `input` string or sends a
non-string value, Codex should return an error tool result for that `tool_use`
id and the freeform handler should not run.

OpenAI hosted tools such as `web_search` and `image_generation` are not
Claude-executable tools in this smoke path unless a future Claude-specific
handler-backed implementation is added. They should not appear in Claude
request `tools`.

## Run the TUI

From the repository root:

```bash
bash test/claude-wire-api/run-tui.sh
```

The script launches the interactive `codex` TUI with `CODEX_HOME` set to this
directory. To use a different binary path, pass `AICODEX_BIN`:

```bash
AICODEX_BIN=/path/to/aicodex bash test/claude-wire-api/run-tui.sh
```
