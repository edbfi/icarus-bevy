# Scratch rules

`work/` is never tracked. It holds the cases behind the evidence index, so it
is pruned by rule, not by size or age. `tools/work_gc.py` applies the mechanical
part; deciding what a case still means is reviewed. Preservation limits in
`.agents/rules/project.md` and the evidence rules in
`.agents/rules/docs.md` take precedence over everything here.

## Layout

- One directory per bounded case: `work/<topic>-<YYYYMMDD>/`, holding its
  scope, commands, notes, receipts and exported REA bundle. Logs and outputs
  belong inside their case, never loose at the top level.
- Each new case writes `case.json` at creation: `{"state": "open", "owner":
  "<agent or session>", "purpose": "<one line>"}`, and sets `state` to `closed`
  when its session closes. `open` tells cleanup to leave the case alone.
- Cases without `case.json` predate this rule and are treated as closed.
- `work/.gc/log.jsonl` records every pruned path; it is not a case.

## Reproducible scratch

What the original inputs and a case's own scripts recreate, and nothing else:

- Ghidra projects (`<name>.gpr`, `<name>.rep`, lock files).
- Wine/Proton prefixes (a directory holding `version` and `pfx/system.reg`).
- Caches (`CACHEDIR.TAG` directories and directories named `.cache`).
- Byte-identical copies, of at least 1 MiB, of files under `icarus-orig/`,
  `icarus-client-orig/` or `reference-data/`.

Scope files, notes, receipts, bundles, logs, private save copies, build
snapshots and authored runtime state are not reproducible scratch. They stay
until their case is deliberately retired.

## Routines

1. Session close: set the case's `case.json` state to `closed`, index what
   rows will cite (`docs.md` rule 2), stop the case's processes and run
   `python3 tools/work_gc.py` to see what it left behind.
2. Milestone: run `python3 tools/work_gc.py --prune`, then
   `python3 tools/work_gc.py --check`, which fails while eligible scratch
   remains. Review the reported signals:
   - scratch held by an active evidence row: narrow a directory row to the
     digested files the findings cite, superseding it (`docs.md` rule 7), or
     retire it; prune on the next run;
   - unindexed cases: keep them, or retire one by deleting it once nothing
     cites its path;
   - loose top-level files: move them into their case.
3. Before deleting anything else in `work/`, grep the path in `evidence/`,
   `docs/` and `.agents/`, and retire every active row that names it first.

## Never

- Delete a path that equals, contains or lies inside an active `local-private`
  evidence row's path.
- Prune a case that is open, used by a running process or changed within the
  settle window (6 hours by default).
- Delete by age or size alone, or with blanket `rm -rf work/*` or `git clean`.
- Prune another agent's open case.
