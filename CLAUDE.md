# Project instructions

Build an independent native Linux ICARUS dedicated server in Rust and Bevy:
stock-client compatibility, player-state save import, reliability and
performance for a pinned official release. The replacement contains no original
assets, data tables, binaries, code or Proton. Today it is a headless foundation;
compatibility is not demonstrated. Offline research has resumed; original-runtime
and connected-client experiments retain their separate current-scope gates.

## Read first

Every session: this file, `docs/linux-dedicated-server-plan.md` (goals,
milestone state, ordered next tasks) and `python3 -m tools.memory status`
(without the service it prints a journal-only summary and says so).
Rules in `.agents/rules/` are authoritative; this file summarizes them. Then
load the entry set for the task and check `docs/gates.md` before acting.

Project knowledge (findings, unknowns, bugs, leads, reports, compatibility,
references, releases, evidence) lives in the memory service and is read and
written only through `python3 -m tools.memory` (`memory`) or its `memory` MCP
server, following `.agents/rules/memory.md`. Fetch a known ID with
`memory get F-487` (also `M-147/ev_<12 hex>`), search with
`memory recall "question"`, list a topic's records with `memory area <area>`
(migrated unknowns carry no area; `memory status` lists every open one,
`memory area` those its records cite). Never read or edit `memory/journal/`
by hand. Without the service, `memory get --offline ID` reads the journal. A
branch or worktree older than the cut-over (it still has `docs/ledgers/` or
`evidence/`) is salvaged from its pre-merge tip before it
merges `main` ([runbook](docs/runbooks/memory-service.md#worktrees-that-predate-the-cut-over)).

| Task | Entry set |
| --- | --- |
| Save-format research | `.agents/rules/project.md`, `docs/gates.md`, `memory area save-format` and `memory area save-format-classes`, `memory get REF-016` (supplied-save class audits) and the relevant format reference (REF-007 to REF-011), `.agents/skills/reverse-engineer-anything/SKILL.md` with the `rea` MCP server |
| C001 research | as save-format, with `memory area save-format-c001` and `memory get REF-015` |
| Network research | `.agents/rules/project.md`, `docs/gates.md`, `memory area network` (also `network-controller`, `network-pawn-state`, `network-presence`), `memory get REF-014` (control stores) and the relevant protocol reference (REF-001 to REF-006), `.agents/skills/reverse-engineer-anything/SKILL.md` with the `rea` MCP server |
| Native server (Rust/Bevy) | `.agents/rules/project.md`, `docs/architecture.md`, `memory area foundation` and `memory get REF-012`, `docs/runbooks/native-server.md`, `.agents/skills/bevy/SKILL.md` |
| Knowledge and documentation | `.agents/rules/memory.md`, `docs/README.md`, `docs/runbooks/memory-service.md` |

For any reverse-engineering work (binary/package inspection, decompilation,
protocol recovery, runtime observation or version comparison), load the
`reverse-engineer-anything` skill (`.agents/skills/reverse-engineer-anything/SKILL.md`)
and use the `rea` MCP server as the primary interface (`docs/runbooks/rea-tooling.md`).
On Ghidra `unsupported_version`, follow "Ghidra drift" in `.agents/rules/project.md`.
Use `.agents/rea/rea.sh` or direct tools only for capabilities REA lacks or a
diagnosed provider/resource limitation, record the reason, and export
`work/<case>/rea-bundle.json` at session close. Project-authored Python uses
`basedpyright-python-gate`.

## Research and preservation

- Never manufacture evidence or approval. Compatibility, reproduction and
  performance claims need evidence records.
- Resuming research needs explicit user authorization. Original-server, client
  and original-code experiments also need current authorization, scope, session
  and cleared gates. Historical records in `docs/history/` grant nothing.
- Input parity: the native server may need no more than the official dedicated
  server needs. Its free public files may be fetched at install time; the game
  client never; needing nothing official is better still (F-828).
- Original materials are research inputs only. Save import reads player-created
  state only.
- Preserve original inputs, private saves, evidence, provenance, licenses and
  user changes. Never delete `icarus-orig/`, `icarus-client-orig/`,
  `reference-data/`, any path `memory protected-paths` lists or another agent's
  files.
- Keep scratch under `work/`, one case per directory with a `case.json`
  (`.agents/rules/work.md`). Bound every experiment and its resources; clean up
  owned experiments on timeout. Delete only owned reproducible scratch; never
  blanket cleanup or deletion by age. Coordinate file ownership; the lead
  integrates, commits and pushes.

## Knowledge and documents

Record claims about originals, measured native properties and decisions that
depend on them as memory records; record receipts and one REA bundle per session
as evidence (`memory evidence add`). Cite permanent IDs where code or documents
depend on them, for example `(F-1162)` or `M-NNN/ev_<12 hex>`; a new citation
names an accepted or same-branch record and never a retired one. Only records
`current` for the pinned release are established for it; work built on any
other partition is recorded as carried from that scope. Follow the session-close,
engineering-loop and milestone procedures of `.agents/rules/memory.md`; commit
the journal segments with the work. A pull request that adds a decision
finding or changes `docs/gates.md` goes to the owner. Plan, gates,
architecture and runbooks stay in Git; `docs/history/` and
`docs/research/archive/` are frozen. The
[memory service runbook](docs/runbooks/memory-service.md) covers install,
backup, restore and outages.

## Validate and deliver

Storage: follow [storage rules](.agents/rules/storage.md) and the
[reporting runbook](docs/runbooks/storage.md). Keep full client/server inputs
local; the owner's development allowance (M-1360) permits bounded growth, never
skipped cleanup; report cleanup candidates for approval before removing research data.

```bash
prek run --all-files --hook-stage manual   # full suite
python3 -m tools.memory check --ci          # journal, citations, forks
python3 tools/docs_check.py                 # links and document paths
```

Fix failures; never bypass hooks or weaken typing. Commit tested increments with
Conventional Commits and `git commit -s`; push the feature branch and verify
delivery. Never force-push, rewrite existing commits or merge into `main` as
routine sync.
