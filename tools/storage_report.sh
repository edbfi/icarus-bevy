#!/usr/bin/env bash
# Read-only storage inventory; never applies the cleanup it reports.
set -euo pipefail
repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
cd -- "$repo_root"
printf 'Storage snapshot (UTC): '
date -u +%FT%TZ
df -B1 --output=source,size,used,avail,pcent,target -- .
printf '\nAllocated bytes (hardlinks counted once across listed roots):\n'
paths=(work target .git)
for path in icarus-* reference-data; do
    [[ ! -d "$path" ]] || paths+=("$path")
done
existing=()
for path in "${paths[@]}"; do
    [[ ! -e "$path" ]] || existing+=("$path")
done
du -sx -B1 -- "${existing[@]}"
printf '\nWorktrees (other checkout targets also consume space):\n'
git worktree list --porcelain
targets=()
while IFS= read -r -d '' field; do
    if [[ "$field" == 'worktree '* ]]; then
        checkout=${field#worktree }
        [[ ! -d "$checkout/target" ]] || targets+=("$checkout/target")
    fi
done < <(git worktree list --porcelain -z)
if ((${#targets[@]})); then
    printf '\nWorktree target allocation (overlaps main target above):\n'
    du -sx -B1 -- "${targets[@]}"
fi
if [[ -f "$HOME/.t3/userdata/logs/boot-service.log" ]]; then
    printf '\nT3 boot log allocation:\n'
    du -B1 -- "$HOME/.t3/userdata/logs/boot-service.log"
fi
printf '\nResearch scratch review; no deletion authorized:\n'
timeout 900 python3 -B tools/work_gc.py
