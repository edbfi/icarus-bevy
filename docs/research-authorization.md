# Offline reference investigation approval

Historical record: the reverse-skill package was removed on 2026-10-12. Its
commands and package requirements below are preserved as approval history, not
current instructions. AGENTS.md links identify the former upstream revision.

Status: approved by the user on 2026-10-04 for the exact offline batch below and
read-only inventory/hash analysis of `icarus-orig/`. Other target activities remain
outside this approval.

The vendored [AGENTS.md](https://github.com/zhaoxuya520/reverse-skill/blob/cab634bd855fc287f6e420c1f36fd1a6b9245960/AGENTS.md) requires explicit
approval of repository scripts and their side effects. Its scope contract also
requires target authorization before analysis.

## Proposed first batch

Run these commands from the project root only after approval:

```bash
bash .agents/vendor/reverse-skill/skills/scripts/master-route.sh \
  --hint "game reverse ICARUS dedicated server" --project-root "$PWD"
bash .agents/vendor/reverse-skill/skills/scripts/case-init.sh \
  --hint "game reverse ICARUS dedicated server" \
  --case-name icarus-server-baseline --project-root "$PWD" \
  --auth-granted --auth-basis own_system \
  --evidence-of-auth "User approval of docs/research-authorization.md" \
  --in-scope-asset "$PWD/icarus-orig" --network-profile offline
bash .agents/vendor/reverse-skill/skills/scripts/case-guard.sh \
  --case-root work/icarus-server-baseline
python3 tools/reference_inventory.py --root icarus-orig \
  --output work/icarus-server-baseline/evidence/inventory.json
```

Approval covers reading and hashing files under `icarus-orig/`, recording file
sizes and paths, and creating routing records, case scope, timeline, work items,
and the inventory under ignored `work/`. It permits publishing sanitized metadata
summaries in `docs/`. Original files remain unchanged. There are no downloads,
target processes, network connections, service starts, or client-global changes.

After case initialization, explicitly record exclusions and permitted activities
in `scope.md`, and rerun the guard before inventory collection. This batch excludes
executing the original server, packet capture, instrumentation, asset extraction,
and decompilation. Present concrete commands for those phases separately.

The inventory is reusable infrastructure, not an implementation of ICARUS
compatibility. Directory entries and hashes are evidence; engine/protocol versions
and root causes still require investigation.
