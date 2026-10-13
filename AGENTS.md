# Project instructions

Build an independent native Linux ICARUS dedicated server in Rust and Bevy:
stock-client compatibility, player-state save import, reliability and
performance for a pinned official release. The replacement contains no original
assets, data tables, binaries, code or Proton. Today it is a headless foundation;
compatibility is not demonstrated. Research is paused until the user resumes it.

## Read first

- All `.agents/rules/*.md`. They are authoritative; this file summarizes them.
- `docs/README.md` (document map), `docs/implementation-status.md` (checkpoint),
  `docs/gates.md` (preconditions). Check applicable gates before acting.
- The plan section, digest and existing `F-`/`U-`/`M-` rows for your area, before
  repeating work: `docs/linux-dedicated-server-plan.md`, `docs/research/<area>/`,
  `docs/ledgers/`.
- Bevy work: `.agents/skills/bevy/SKILL.md`. Reverse engineering:
  `.agents/skills/reverse-engineer-anything/SKILL.md` with the `rea` MCP server;
  fallbacks per `project.md`. Project-authored Python: `basedpyright-python-gate`.

## Research and preservation

- Never manufacture evidence or approval. Compatibility, reproduction and
  performance claims need evidence rows.
- Resuming research needs explicit user authorization. Original-server, client
  and original-code experiments also need current authorization, scope, session
  and cleared gates. Historical records in `docs/history/` grant nothing.
- Original materials are research inputs only. Save import reads player-created
  state only.
- Preserve original inputs, private saves, evidence, provenance, licenses and
  user changes. Never delete `icarus-orig/`, `icarus-client-orig/`,
  `reference-data/`, manifest `local-private` paths or another agent's files.
- Keep scratch under `work/`. Bound every experiment and its resources; clean up
  owned experiments on timeout. Delete only owned reproducible scratch; never
  blanket cleanup or deletion by age. Coordinate file ownership; the lead
  integrates, commits and pushes.

## Documentation

| Content | Location |
| --- | --- |
| Goal, milestones, gates | `docs/linux-dedicated-server-plan.md` |
| What works, what does not, next tasks | `docs/implementation-status.md` |
| Preconditions that block work | `docs/gates.md` |
| Design constraints | `docs/architecture.md` |
| Procedures and commands | `docs/runbooks/` |
| Durable format and protocol facts | `docs/specs/<area>/` |
| Conclusions, questions, bugs, compatibility | `docs/ledgers/` (`F-`, `U-`, bug and matrix rows) |
| Research digests, one per question cluster | `docs/research/<area>/` |
| Evidence artifacts | `evidence/manifest.md` (`M-`) |
| Frozen history | `docs/history/`, `docs/research/archive/` |

- Ledger IDs are permanent: never delete or renumber; mark superseded or resolved.
- Rewrite current documents in place. No dated headings, session narrative, tool
  output or restated background. Prose cites IDs, not hashes, clock times or
  `work/` paths; `docs.md` lists the exceptions (code, runbooks, gates, cells).
- Respect byte caps and fixed headings. Condense near a cap; never split to
  evade it. Condensation keeps every observation, qualification and provenance.
- A research session ends with finding rows, unknown rows and the matching
  digest. Each newly cited `ev_` identifier needs an exported Evidence bundle
  under `work/` and a manifest row. Update affected current documents.
- At milestones, run the checklist in `docs.md`.

## Validate and deliver

```bash
prek run --all-files --hook-stage manual   # full suite
python3 tools/docs_lint.py                  # documentation only
```

Fix failures; never bypass hooks or weaken typing. Commit tested increments with
Conventional Commits and `git commit -s`; push the feature branch and verify
delivery. Never force-push, rewrite existing commits or merge into `main` as
routine sync.
