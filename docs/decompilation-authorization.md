# Bounded headless decompilation batch

Historical record: the reverse-skill package was removed on 2026-10-12. Its
commands and package requirements below are preserved as approval history, not
current instructions. AGENTS.md links identify the former upstream revision.

For current work, follow the [canonical plan](linux-dedicated-server-plan.md)
and [implementation status](implementation-status.md). This record does not
resume research or extend an execution window.

Status: approved by the user on 2026-10-04, including both Java helper scripts.
Static analysis identified diagnostic string addresses for failed prospect loading
and incompatible network versions. These are leads, not bugs.

## Historical proposed command

After adding offline decompilation to the existing scope and rerunning its guard,
execute from the project root:

```bash
bash .agents/vendor/reverse-skill/skills/scripts/case-guard.sh \
  --case-root work/icarus-server-baseline
mkdir -p work/icarus-server-baseline/ghidra
JAVA_HOME="$(brew --prefix openjdk@21)/libexec" \
  timeout 600s "$(brew --prefix ghidra)/libexec/support/analyzeHeadless" \
  "$PWD/work/icarus-server-baseline/ghidra" IcarusBaseline \
  -import "$PWD/icarus-orig/Icarus/Binaries/Win64/IcarusServer-Win64-Shipping.exe" \
  -analysisTimeoutPerFile 180 -max-cpu 2 \
  -scriptPath "$PWD/tools/ghidra" \
  -preScript OfflineAnalysis.java \
  -postScript ExportCompatibilityAnchors.java \
  "$PWD/work/icarus-server-baseline/ghidra-export" \
  1449f1550 1449f6660 14546d780 \
  -log "$PWD/work/icarus-server-baseline/ghidra-analysis.log" \
  -scriptlog "$PWD/work/icarus-server-baseline/ghidra-script.log"
```

Effects: launch headless Java/Ghidra and its decompiler helper, import the local
executable into an ignored analysis database, analyze for at most 180 seconds,
and export references plus at most six functions (15 seconds each). Overall
timeout is ten minutes. Ghidra may use its normal local compilation/cache and
preferences directories. PDB symbol-server analyzers are disabled before analysis.
No symbol download, target execution, original modification, asset extraction,
network experiment, GUI or client-global configuration change is authorized.

Generated decompilation stays in ignored `work/`, never replacement source. Publish
only independent behavioral explanations and evidence locations under `docs/`.
Incomplete analysis or missing cross-references must be reported, not interpreted
as absence of behavior.

Approval is required by the vendored [AGENTS.md](https://github.com/zhaoxuya520/reverse-skill/blob/cab634bd855fc287f6e420c1f36fd1a6b9245960/AGENTS.md):
"Explicit user approval is required before running any repository script."
This includes the two new Java scripts above; their source is available for review.
