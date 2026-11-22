# REA tooling and reference environments

Restore REA and the three reference environments. Usage rules:
[project rules](../../.agents/rules/project.md).

## REA runtime

[REA](https://github.com/morluto/rea) is pinned to `rea-agents@6.9.0` in
`.agents/rea/package-lock.json`. Codex uses `.codex/config.toml`, Claude
`.mcp.json`; both use `.agents/skills/reverse-engineer-anything/` (linked under
`.claude/skills/`). Restore and check from the repository root:

```bash
npm ci --prefix .agents/rea --cache .agents/rea/.npm-cache
.agents/rea/rea.sh doctor --provider ghidra --json
```

Restart the client after configuration changes and accept project/MCP trust
prompts. Do not run global setup; preserve locks, skill provenance and licenses.

Doctor checks readiness; MCP `binary_session` checks the connection. User-wide
registrations are unnecessary. Follow "Ghidra drift" in `.agents/rules/project.md`
on `unsupported_version`. `rea.sh` prefers
`~/.local/share/rea-ghidra/current` over Homebrew's Ghidra. It selects the
declared minimum JDK, unversioned Homebrew JDK, or closest installed newer
`openjdk@N`, in that order; explicit `JAVA_HOME` takes precedence.

Ghidra bytes are loader-adjusted; REA requires default auto-analysis. Physical
metadata needs a reviewed bounded fallback and independent input hashes.
Current-reader refusal does not prove legacy-bundle corruption: preserve bytes
and producing-version evidence (M-975). Never repair identifiers or overwrite
frozen bundles.

Use `Brewfile`'s installed Homebrew radare2, headless Ghidra and Wireshark CLI.
Check `tshark --version` separately; GUI failure does not rule out headless use.
For a justified direct fallback, resolve `rea.sh`'s JDK/Ghidra paths in scope:

```bash
JAVA_HOME="$SCOPED_JDK" "$SCOPED_GHIDRA/support/analyzeHeadless" <arguments>
```

PE/PDB readers and other authored helpers: [research helpers](research-helpers.md).

Run `tools/ghidra/TraceBoundedFunctions.java NEW_DIRECTORY START:END_EXCLUSIVE`
with `-noanalysis`. Require `EXPORT_FINISHED.txt`, no `FAILED.txt`, physical-byte
checks and inspected statuses. Pseudocode and listing installation are disabled;
userops, overrides and noncanonical context refuse admission (M-977, M-979).

For a separate raw program, use only the hashed physical fragment with
`-loader BinaryLoader -processor x86:LE:64:default -cspec windows`, its original
VA and positive hexadecimal length. Provider hashing reads the whole input.
Retain PE metadata/prior partial traces; the raw block proves no extent (M-985).

`DecompileFlowFunctions.java` and `ListFlowInstructions.java` use forward
windows; completion proves no whole-function coverage. `struct_ops_methods`
selects inline `TCppStructOps` PDB symbols.

## Evidence at session close

The session bundle is canonical; its evidence record in the memory journal
indexes it ([memory rules](../../.agents/rules/memory.md), Session close).

1. Call the MCP `export_evidence_bundle` tool with an absolute `path` ending in
   `work/<case>/rea-bundle.json`. It reports the record and unknown counts.
2. `python3 -m tools.memory evidence add work/<case>/rea-bundle.json --kind
   rea-bundle --label "<what the session observed>"`. The tool allocates the
   `M-NNN`, hashes the file and lists the bundle's records and members.
3. Cite `M-NNN/ev_<12 hex>` in findings and `M-NNN/unk_<12 hex>` in unknowns;
   never paste a full identifier. Proposing a record checks each selector
   against the bundle's members (export layout: F-494).
4. `python3 -m tools.memory verify M-NNN` checks the digest and membership; the
   pre-commit hook verifies every evidence record in staged segments.

Read summary/exact filters, never whole bundles. Get the full ID with
`python3 -m tools.memory get M-NNN/ev_<12 hex>`:

```bash
.agents/rea/rea.sh inspect-evidence-bundle \
  '{"path":"/abs/work/<case>/rea-bundle.json","detail":"summary","filters":{"evidence_id":"ev_<64 hex>"}}'
```

In a live session, use `get_evidence_bundle`, `detail: "summary"` and the same
filters without exporting.

## Private evidence backup

The journal records `local-private` evidence; its bytes stay on the analyst
machine. Back up every protected path outside the repository to
`ICARUS_EVIDENCE_BACKUP`, then prove a scratch-clone restore:

```bash
(set -o pipefail; python3 -m tools.memory protected-paths |
  rsync -a -r --relative --files-from=- ./ "$ICARUS_EVIDENCE_BACKUP/")
git clone -q . "$RESTORE_CHECK" && rsync -a "$ICARUS_EVIDENCE_BACKUP/" "$RESTORE_CHECK/"
(cd "$RESTORE_CHECK" && python3 -m tools.memory verify --all)
```

`--files-from` needs `-r` for recursion. A nonzero exit from `protected-paths`
means unreadable input (named on stderr): treat the backup as incomplete. In
the restore check `verify` resolves paths against the clone; `unavailable` or
`mismatched` names what the backup lacks. Back up after every session adding
evidence; retire a record before deleting its bytes.

## Reference environments

Maintain three environments:

1. Official server under GE-Proton using the user's Pelican reference at
   `/home/gvps/github/edbfi/pelican-eggs/games-steamcmd/icarus-proton`.
2. Official Windows server using the user's home machine for behavior and fault attribution.
3. Native replacement on Linux with matched workloads and resource limits.

The inspected Pelican egg uses `xvfb-run`/Proton, install app 2089300, runtime
game 1149460 and `Icarus/Saved`. Pin container digest, Proton version, official
build and configuration; disable automatic updates in experiments. Never run its container
installer unchanged on the host.
