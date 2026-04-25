# Anthropic Manual Test Config

This document describes a minimal manual test setup for the native Anthropic
wire protocol support.

## Goal

Use this config to manually verify:

- text streaming
- reasoning streaming
- function-tool round trips
- `local_shell` round trips
- `apply_patch` round trips

## Prerequisites

- An Anthropic API key with access to Claude models
- A local build of the current branch
- A writable test workspace

## Minimal Config

Add the following to `~/.codex/config.toml`:

```toml
[model_providers.anthropic]
name = "Anthropic"
base_url = "https://api.anthropic.com"
env_key = "ANTHROPIC_API_KEY"
wire_api = "anthropic"

model_provider = "anthropic"
model = "claude-sonnet-4-5"
approval_policy = "never"
sandbox_mode = "danger-full-access"
include_apply_patch_tool = true
```

Then export the API key:

```bash
export ANTHROPIC_API_KEY="..."
```

## Suggested Manual Checks

### 1. Text streaming

```bash
cargo run -p codex-cli -- exec --skip-git-repo-check -C /tmp "say hello in one short sentence"
```

Expected result:

- assistant text is streamed
- turn completes normally

### 2. Reasoning streaming

```bash
cargo run -p codex-cli -- exec --skip-git-repo-check -C /tmp "reason step by step about whether 17 is prime, then answer in one line"
```

Expected result:

- assistant completes successfully
- if your surface exposes reasoning deltas, they should appear during the turn

### 3. Function tool call

```bash
cargo run -p codex-cli -- exec --skip-git-repo-check -C /tmp "tell me the current UTC time"
```

Expected result:

- the model emits a tool call such as `time`
- tool output is sent back
- the assistant returns a final answer

### 4. Local shell tool call

Run inside a scratch directory:

```bash
mkdir -p /tmp/codex-anthropic-manual
cd /tmp/codex-anthropic-manual
cargo run -p codex-cli -- exec --skip-git-repo-check -C /tmp/codex-anthropic-manual "run local shell and tell me the current working directory"
```

Expected result:

- the model calls `local_shell`
- the shell command runs
- the assistant returns the directory path

### 5. Freeform apply_patch tool call

Run inside a scratch directory:

```bash
mkdir -p /tmp/codex-anthropic-patch
cd /tmp/codex-anthropic-patch
cargo run -p codex-cli -- exec --skip-git-repo-check -C /tmp/codex-anthropic-patch "use apply_patch to create a file named hello.txt containing exactly hello"
```

Expected result:

- the model calls `apply_patch`
- `hello.txt` is created
- the assistant confirms completion

Verify:

```bash
cat /tmp/codex-anthropic-patch/hello.txt
```

Expected output:

```text
hello
```

## Known Limits in This Phase

- `ImageGeneration` / `WebSearch` / `ToolSearch` run through provider-side tool semantics rather than a local Codex tool backend.
- Anthropic-specific output-schema auto-repair is not implemented.

## Debug Tips

If manual testing fails:

- confirm `model_provider = "anthropic"` is active
- confirm `wire_api = "anthropic"` is set on the provider
- confirm `ANTHROPIC_API_KEY` is exported in the same shell
- use a writable scratch directory for `local_shell` and `apply_patch`
- if the provider returns auth failures, verify the key is valid and not empty
