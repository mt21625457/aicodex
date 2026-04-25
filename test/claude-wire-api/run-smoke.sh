#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/../.." && pwd)"
CODEX_HOME="${SCRIPT_DIR}"

if [[ -f "${SCRIPT_DIR}/.env" ]]; then
  set -a
  # shellcheck disable=SC1091
  source "${SCRIPT_DIR}/.env"
  set +a
fi

if [[ -z "${ANTHROPIC_API_KEY:-}" ]]; then
  echo "ANTHROPIC_API_KEY is not set. Copy .env.example to .env and fill it in." >&2
  exit 1
fi

cd "${REPO_ROOT}/codex-rs"

CODEX_HOME="${CODEX_HOME}" \
  CARGO_TARGET_DIR=target-claude-smoke \
  cargo run -p codex-exec -- \
  --skip-git-repo-check \
  --ephemeral \
  --cd "${REPO_ROOT}" \
  --json \
  "Reply with exactly: claude-wire-api-smoke-ok"
