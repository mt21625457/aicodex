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

