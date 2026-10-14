# Project instructions

Build an independent native Linux ICARUS dedicated server in Rust and Bevy:
stock-client compatibility, player-state save import, reliability and
performance for a pinned official release. The replacement contains no original
assets, data tables, binaries, code or Proton. Today it is a headless foundation;
compatibility is not demonstrated. Offline research has resumed; original-runtime
and connected-client experiments retain their separate current-scope gates.

## Read first

Every session: this file, `docs/README.md` and `docs/implementation-status.md`.
Rules in `.agents/rules/` are authoritative; this file summarizes them. Then
load the entry set for the task, check `docs/gates.md` before acting, and fetch
ledger rows by ID (`grep -w F-487 -r docs/ledgers`), never whole ledgers.

| Task | Entry set |
| --- | --- |
| Save-format research | `.agents/rules/project.md`, `docs/gates.md`, `docs/research/save-format/save-class-audits.md`, the relevant `docs/specs/save-format/` file, `.agents/skills/reverse-engineer-anything/SKILL.md` with the `rea` MCP server |
| C001 research | as save-format, with `docs/research/save-format/c001-constructor.md` |
| Network research | `.agents/rules/project.md`, `docs/gates.md`, `docs/research/network/control-stores.md`, the relevant `docs/specs/network/` file, the reverse-engineering skill |
| Native server (Rust/Bevy) | `.agents/rules/project.md`, `docs/architecture.md`, `docs/research/foundation/foundation.md`, `docs/runbooks/native-server.md`, `.agents/skills/bevy/SKILL.md` |
| Documentation | `.agents/rules/docs.md`, `docs/README.md` |

REA fallbacks follow `project.md`. Project-authored Python uses
`basedpyright-python-gate`.

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

`.agents/rules/docs.md` holds the layout, read tiers, caps and rules. In short:
admit a finding only for a claim about the original, a measured native property
or a conclusion something depends on (receipts are manifest rows); at session
close edit only what changed: digest, spec, finding rows (consolidate chains into
a summary row), `unknowns.md`, `gates.md`, manifest. Cite IDs instead of
restating claims; near a cap, split by topic or retire rows, never strip spaces.
IDs are permanent. Run the milestone checklist in `docs.md` at milestones.

## Validate and deliver

```bash
prek run --all-files --hook-stage manual   # full suite
python3 tools/docs_lint.py                  # documentation only
```

Fix failures; never bypass hooks or weaken typing. Commit tested increments with
Conventional Commits and `git commit -s`; push the feature branch and verify
delivery. Never force-push, rewrite existing commits or merge into `main` as
routine sync.
