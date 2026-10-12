#!/usr/bin/env bash
# Offline laboratory entry point; Python requires an explicit current case/session.
set -euo pipefail
script_dir="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd -P)"
exec python3 "$script_dir/run_reference_offline.py" "$@"
