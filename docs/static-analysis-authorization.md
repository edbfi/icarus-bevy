# Historical offline static analysis batch

Historical record: the reverse-skill package was removed on 2026-10-12. Its
commands and package requirements below are preserved as approval history, not
current instructions. The upstream `AGENTS.md` link identifies the former revision.

For current work, follow the [canonical plan](linux-dedicated-server-plan.md)
and [implementation status](implementation-status.md). This record does not
resume research or extend an execution window.

Status: approved by the user on 2026-10-04. This extends the inventory scope; it
does not authorize running the original server or copying assets into the replacement.

The inventory identified `Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe`
(108,067,840 bytes, SHA-256
`67cfc915d9d085b758057e2960d4a2018fb121123af1329c25ef7aa58b960104`).

## Historical commands proposed for approval

Run from the repository root after recording static binary inspection, strings
inspection, and build-manifest reading in the existing offline case scope:

```bash
bash .agents/vendor/reverse-skill/skills/scripts/case-guard.sh \
  --case-root work/icarus-server-baseline
mkdir -p work/icarus-server-baseline/static
/home/linuxbrew/.linuxbrew/bin/rabin2 -Ij \
  icarus-orig/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe \
  > work/icarus-server-baseline/static/server-info.json
/home/linuxbrew/.linuxbrew/bin/rabin2 -ij \
  icarus-orig/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe \
  > work/icarus-server-baseline/static/server-imports.json
/home/linuxbrew/.linuxbrew/bin/rabin2 -lj \
  icarus-orig/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe \
  > work/icarus-server-baseline/static/server-libraries.json
/home/linuxbrew/.linuxbrew/bin/rabin2 -zj \
  icarus-orig/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe \
  > work/icarus-server-baseline/static/server-strings.json
cp icarus-orig/steamapps/appmanifest_2089300.acf \
  work/icarus-server-baseline/static/appmanifest_2089300.acf
```

Effects: read the authorized executable and manifest, create JSON/text analysis
artifacts inside ignored `work/`, inspect those artifacts, and publish sanitized
metadata/behavioral leads under `docs/`. No original files change. No target
execution, downloads, network, services, GUI, global configuration or asset reuse.

Decompilation and dynamic experiments remain separate subsequent batches. This
approval is required by the vendored [AGENTS.md](https://github.com/zhaoxuya520/reverse-skill/blob/cab634bd855fc287f6e420c1f36fd1a6b9245960/AGENTS.md):
"Explicit user approval is required before running any repository script."
Target scope remains a separate requirement.
