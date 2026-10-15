# REA tooling and reference environments

How to restore and check the project-local reverse-engineering runtime and the
three reference environments. Rules for when to use REA are in
[`.agents/rules/project.md`](../../.agents/rules/project.md).

## REA runtime

The project uses [REA](https://github.com/morluto/rea), pinned as
`rea-agents@6.5.0` in `.agents/rea/package-lock.json`. Its wrapper and MCP
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

The scoped Ghidra doctor check reports provider readiness. Its informational
checks inspect user-wide registrations and may report them missing because this
installation deliberately uses project configuration. Verify registration with
`codex mcp get rea` and `claude mcp get rea`; verify the live connection with the
MCP `binary_session` tool. REA 6.5.0's skill metadata lists 207 MCP tools; its
scoped doctor reported the existing Ghidra 12.1.4/JDK 21 provider healthy on
2026-10-15 (M-563). The connected 6.4.0 catalog of 170 tools was aligned (F-432); recheck
alignment with `binary_session` after restarting on 6.5.0. Ghidra byte reads require default
auto-analysis and return loader-adjusted Program memory. For exact physical
metadata work, record that capability/effect gap and use a reviewed bounded
fallback; connected readiness does not qualify target bytes.

Use the installed Homebrew tools recorded in `Brewfile`: radare2, headless Ghidra
and Wireshark CLI utilities. Check `tshark --version` separately. A GUI failure
does not mean headless Ghidra is broken. For a justified direct Ghidra fallback,
resolve the arguments before presenting an analysis command:

```bash
JAVA_HOME="$(brew --prefix openjdk@21)/libexec" \
  "$(brew --prefix ghidra)/libexec/support/analyzeHeadless" <arguments>
```

For a targeted decompilation of a few PDB-named functions in a large image,
import with `-noanalysis` and run `tools/ghidra/DecompileFlowFunctions.java`
with the starts: it disassembles each start by flow within 64 KiB, lets Ghidra
compute the body (the PE loader already creates one-byte functions at exception
records, so guessed end ranges overlap) and decompiles with a 30-second cap.
When pseudocode drops a call argument, `tools/ghidra/ListFlowInstructions.java`
lists the same flow (jumps and fallthrough, never calls, at most 4096
instructions per start) with address, bytes and instruction text. Header-inline
struct methods have no class-method symbol; `tools/pdb_public_symbols.cjs`
selects their `TCppStructOps` instance through `struct_ops_methods` pairs.

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

`--files-from` disables recursion unless `-r` is given. A dry run (`-n --stats`)
on 2026-10-14 listed 224 paths and about 60 GB, mostly the client copy under
`icarus-client-orig/` and `reference-data/`. Back up after each
session close that adds rows; deleting an artifact still requires its row to be
retired first.

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
