#!/usr/bin/env bash
# Use this repository's pinned REA runtime and existing analysis tools.
set -euo pipefail
rea_root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)"
rea_entry="$rea_root/.agents/rea/node_modules/rea-agents/scripts/rea.mjs"
if [[ ! -f "$rea_entry" ]]; then
  echo 'Install REA first: npm ci --prefix .agents/rea --cache .agents/rea/.npm-cache' >&2
  exit 1
fi
# A Ghidra the installed REA accepts when Homebrew's has moved past it
# ("Ghidra drift" in .agents/rules/project.md).
rea_ghidra_current="${REA_GHIDRA_ROOT:-${XDG_DATA_HOME:-$HOME/.local/share}/rea-ghidra}/current"
if [[ -z "${GHIDRA_INSTALL_DIR:-}" && -f "$rea_ghidra_current/support/analyzeHeadless" ]]; then
  GHIDRA_INSTALL_DIR="$(cd -- "$rea_ghidra_current" && pwd -P)"
  export GHIDRA_INSTALL_DIR
fi
if command -v brew >/dev/null 2>&1; then
  brew_prefix="$(cd -- "$(dirname -- "$(command -v brew)")/.." && pwd)"
  if [[ -z "${GHIDRA_INSTALL_DIR:-}" ]]; then
    rea_ghidra="$brew_prefix/opt/ghidra"
    [[ -f "$rea_ghidra/libexec/support/analyzeHeadless" ]] || rea_ghidra="$(brew --prefix ghidra 2>/dev/null || true)"
    if [[ -f "$rea_ghidra/libexec/support/analyzeHeadless" ]]; then
      export GHIDRA_INSTALL_DIR="$rea_ghidra/libexec"
    fi
  fi
  if [[ -z "${JAVA_HOME:-}" ]]; then
    # Use the JDK line the selected Ghidra declares, else Homebrew's newest.
    rea_java_min="$(sed -n 's/^application\.java\.min=//p' "${GHIDRA_INSTALL_DIR:-/nonexistent}/Ghidra/application.properties" 2>/dev/null || true)"
    for rea_java in ${rea_java_min:+"openjdk@$rea_java_min"} openjdk; do
      if [[ -x "$brew_prefix/opt/$rea_java/libexec/bin/java" ]]; then
        export JAVA_HOME="$brew_prefix/opt/$rea_java/libexec"
        break
      fi
    done
  fi
fi
if [[ -z "${REA_TSHARK_COMMAND:-}" ]] && command -v tshark >/dev/null 2>&1; then
  export REA_TSHARK_COMMAND="$(command -v tshark)"
fi
cd -- "$rea_root"
exec node "$rea_entry" "$@"
