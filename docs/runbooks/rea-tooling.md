# REA tooling and reference environments

How to restore and check the project-local reverse-engineering runtime and the
three reference environments. Rules for when to use REA are in
[`.agents/rules/project.md`](../../.agents/rules/project.md).

## REA runtime

The project uses [REA](https://github.com/morluto/rea), pinned as
`rea-agents@6.7.0` in `.agents/rea/package-lock.json`. Its wrapper and MCP
configuration are local to this repository: Codex reads `.codex/config.toml`,
Claude reads `.mcp.json`, and both discover the same skill through
`.agents/skills/reverse-engineer-anything/` and its `.claude/skills/` link.
Restore the locked runtime and check the Ghidra provider from the repository root:

```bash
npm ci --prefix .agents/rea --cache .agents/rea/.npm-cache
.agents/rea/rea.sh doctor --provider ghidra --json
```

Restart Codex and Claude after configuration changes and accept their project/MCP
trust prompts when presented. Do not run upstream global setup; it would change
configuration outside this repository. Preserve the package lock, skill
provenance and third-party licenses when updating.

Doctor reports provider readiness, not a live connection. Verify registration
with `codex mcp get rea` or `claude mcp get rea`, then query MCP
`binary_session`. Missing user-wide registrations are informational for this
project-local setup. M-778 records the earlier healthy 12.1.4/JDK 21 provider;
M-975 records current drift: REA 6.7.0 rejects Ghidra 12.2, whose installation also
needed JDK 25 and a matching native decompiler. Do not relabel installations or
bypass policy. The scoped direct fallback built the matching public source in
an owned overlay. M-973/M-974 validate an owned REA stdio session and authored
fixture through bounded GNU objdump, not Ghidra provider readiness.

Ghidra byte reads return loader-adjusted analysis memory and require default
auto-analysis through REA. Exact physical metadata needs a reviewed bounded
fallback and independent input hashes. A legacy bundle's current-reader refusal
does not prove corruption; preserve its bytes and producing-version evidence
(M-975). Never repair identifiers or overwrite frozen bundles.

Use the installed Homebrew tools recorded in `Brewfile`: radare2, headless Ghidra
and Wireshark CLI utilities. Check `tshark --version` separately. A GUI failure
does not mean headless Ghidra is broken. For a justified direct Ghidra fallback,
resolve the matching JDK and tool paths in its scope before running:

```bash
JAVA_HOME="$SCOPED_JDK" "$SCOPED_GHIDRA/support/analyzeHeadless" <arguments>
```

For exact-range traces, bind public symbols with `tools/pdb_public_symbols.cjs`
and exception metadata with `tools/pe_function_ranges.cjs`; neither proves
ownership or instruction boundaries. Import with `-noanalysis` and run
`tools/ghidra/TraceBoundedFunctions.java NEW_DIRECTORY START:END_EXCLUSIVE`.
It exports bounded instruction/p-code/context records, observed intervals and
partial-flow statuses. Require `EXPORT_FINISHED.txt`, no `FAILED.txt`, then
inspect each status. Pseudocode is disabled; unaudited userops, overrides and
noncanonical execution context reject. Memory hashes need physical-span checks
(M-977, M-979).
The older `DecompileFlowFunctions.java` and `ListFlowInstructions.java` use
forward windows; their completion flags do not prove whole-function coverage.
`struct_ops_methods` selects inline `TCppStructOps` symbols in the PDB helper.

## Evidence at session close

The exported bundle is the canonical record of a session; the repository holds
only its index row and citations (rules in
[`.agents/rules/docs.md`](../../.agents/rules/docs.md)).

1. Call the MCP `export_evidence_bundle` tool with an absolute `path` ending in
   `work/<case>/rea-bundle.json`. It reports the record and unknown counts.
2. Allocate an ID with `python3 tools/docs_lint.py --next-id M` and append one
   record to the highest-numbered shard of its category folder in `evidence/`:
   `kind` `rea-bundle`, the file's `sha256sum`, `records` from the export, and
   in `ev` and `unk` only the IDs that rows cite.
3. Cite `M-NNN/ev_<12 hex>` in finding Evidence cells and `M-NNN/unk_<12 hex>`
   in U- rows; never paste a full identifier.
4. Run `python3 tools/docs_lint.py --local`; it checks the digest and that every
   listed ID is a member of the bundle (export layout: F-494).

Read a bundle by summary and exact filter, never whole. Take the full ID from
the row (`grep -rhw --include='*.jsonl' M-NNN evidence/`), then:

```bash
.agents/rea/rea.sh inspect-evidence-bundle \
  '{"path":"/abs/work/<case>/rea-bundle.json","detail":"summary","filters":{"evidence_id":"ev_<64 hex>"}}'
```

Inside a live session, `get_evidence_bundle` with `detail: "summary"` and the
same filters does the same without exporting.

## Private evidence backup

`local-private` evidence exists only on the analyst machine; Git holds its index
rows, not its bytes. Back up every active path to storage outside the repository
named by `ICARUS_EVIDENCE_BACKUP`, then prove a restore into a scratch clone:

```bash
python3 tools/docs_lint.py --list-local |
  rsync -a -r --relative --files-from=- ./ "$ICARUS_EVIDENCE_BACKUP/"
git clone -q . "$RESTORE_CHECK" && rsync -a "$ICARUS_EVIDENCE_BACKUP/" "$RESTORE_CHECK/"
python3 tools/docs_lint.py --local --root "$RESTORE_CHECK"
```

`--files-from` disables recursion unless `-r` is given. Back up after each session
that adds rows; retire an artifact's row before deletion.

## Reference environments

Maintain three environments:

1. Official server under GE-Proton using the user's Pelican reference at
   `/home/gvps/github/edbfi/pelican-eggs/games-steamcmd/icarus-proton`.
2. Official Windows server using the user's home machine for behavior and fault attribution.
3. Native replacement on Linux with matched workloads and resource limits.

The inspected Pelican egg uses `xvfb-run` and Proton, installation app ID 2089300,
runtime game ID 1149460, and persistent data under `Icarus/Saved`. Pin container
digest, Proton version, official build and configuration; disable automatic
updates in experiments. Do not run its container installer unchanged on the host.
