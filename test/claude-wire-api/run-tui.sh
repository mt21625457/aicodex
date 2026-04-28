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

DEFAULT_BIN="${REPO_ROOT}/codex-rs/target/debug/codex"
AICODEX_BIN="${AICODEX_BIN:-${CODEX_BIN:-${DEFAULT_BIN}}}"

if [[ ! -x "${AICODEX_BIN}" ]]; then
  echo "Codex TUI binary not found or not executable: ${AICODEX_BIN}" >&2
  echo "Build it with: cd ${REPO_ROOT}/codex-rs && cargo build -p codex-cli" >&2
  echo "Or run with: AICODEX_BIN=/path/to/aicodex ${0}" >&2
  exit 1
fi

exec env \
  CODEX_HOME="${CODEX_HOME}" \
  ANTHROPIC_API_KEY="${ANTHROPIC_API_KEY}" \
  "${AICODEX_BIN}" \
  --cd "${REPO_ROOT}" \
  --dangerously-bypass-approvals-and-sandbox
