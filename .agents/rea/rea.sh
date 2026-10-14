#!/usr/bin/env bash
# Use this repository's pinned REA runtime and existing analysis tools.
set -euo pipefail
rea_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
rea_entry="$rea_root/.agents/rea/node_modules/rea-agents/scripts/rea.mjs"
if [[ ! -f "$rea_entry" ]]; then
  echo 'Install REA first: npm ci --prefix .agents/rea --cache .agents/rea/.npm-cache' >&2
  exit 1
fi
if command -v brew >/dev/null 2>&1; then
  if [[ -z "${GHIDRA_INSTALL_DIR:-}" ]]; then
    rea_ghidra="$(brew --prefix ghidra 2>/dev/null || true)"
    if [[ -f "$rea_ghidra/libexec/support/analyzeHeadless" ]]; then
      export GHIDRA_INSTALL_DIR="$rea_ghidra/libexec"
    fi
  fi
  if [[ -z "${JAVA_HOME:-}" ]]; then
    rea_java="$(brew --prefix openjdk@21 2>/dev/null || true)"
    if [[ -x "$rea_java/libexec/bin/java" ]]; then
      export JAVA_HOME="$rea_java/libexec"
    fi
  fi
fi
if [[ -z "${REA_TSHARK_COMMAND:-}" ]] && command -v tshark >/dev/null 2>&1; then
  export REA_TSHARK_COMMAND="$(command -v tshark)"
fi
cd -- "$rea_root"
exec node "$rea_entry" "$@"
