# Documentation rules

Documentation has two surfaces. `tools/docs_lint.py` enforces the mechanical
rules through prek and CI; the rest are reviewed.

## Surfaces

- Current surface: `README.md` and everything under `docs/` except the frozen
  directories. Bounded, rewritten in place, lint-enforced.
- Retained history: `docs/history/` and `docs/research/archive/`. Verbatim,
  append-only, frozen to the lint allowlist. Only a banner and link targets may
  change. New files there require a deliberate allowlist edit.

## Where things go

| Content | Location |
| --- | --- |
| Goal, milestones, exit criteria | `docs/linux-dedicated-server-plan.md` |
| What works, what does not, next tasks | `docs/implementation-status.md` (fixed H2 set) |
| Preconditions that block actions, grouped by action | `docs/gates.md` |
| Operating procedures and commands | `docs/runbooks/` |
| Durable technical findings per format or protocol | `docs/specs/<area>/` (fixed H2 set) |
| One row per conclusion, bug, compatibility item, open question | `docs/ledgers/` |
| Research digests, one per question cluster | `docs/research/<area>/` (fixed H2 set) |
| Evidence artifacts: path, digest, session, class | `evidence/manifest.md` |
| Historical authorizations and reports | `docs/history/`, `docs/research/archive/` |
| How agents work in this repository | `.agents/rules/` only |

## Rules

1. A research session finalizes by updating or adding finding rows in
   `docs/ledgers/findings.md`, updating `docs/ledgers/unknowns.md`, and updating
   the matching digest. Create a new digest only for a new question cluster.
   Export an Evidence bundle under `work/` and add a manifest row whenever a new
   `ev_` identifier is cited. REA sessions are a working copy; the tracked
   unknowns ledger is the record.
2. Current-surface files are rewritten in place. No dated or timestamped
   headings, no appended session notes.
3. Session narrative, tool output and restated background do not enter the
   repository. Conclusions go to ledgers, procedures to runbooks, questions to
   digests. Prose cites `F-`, `U-` and `M-` identifiers instead of hashes,
   clock times or `work/` paths. Code spans, fenced blocks, runbooks, gates and
   ledger cells are exempt.
4. Ledger IDs are permanent. Mark a finding `superseded-by F-NNN`; never delete
   or renumber a row.
5. Caps are in bytes: 16 KB per current-surface file, 64 KB per ledger (128 KB
   for findings), 400 bytes per ledger row, and 160 KB for the current prose surface (every
   current-surface file except the ledgers). A file near its cap follows the
   drop policy (rule 9).
6. Every status, spec and ledger file carries `Last reconciled: YYYY-MM-DD`.
7. Canonical workflow rules live under `.agents/rules/`; `AGENTS.md` summarizes
   and links to them. Historical records, spec acceptance conditions and runbook
   procedures are exempt from the no-duplication rule.
8. Condensing a current document keeps every observation, qualification,
   limitation, scope and outcome retrievable by ID as a ledger row, gate line or
   digest line. The file header cites the commit and path that still hold the
   full text.
9. Drop policy. Near a cap, split by topic (specs, digests, active findings,
   numbered retired findings files) or supersede and retire finding rows.
   Deleting prose that restates an ID-retrievable claim is permitted and
   preferred; rewording a row to fit its cap is permitted. Frozen directories
   are never a destination. Removing whitespace to fit a cap is a lint failure.

## Milestone checklist

1. Reconcile `docs/implementation-status.md` against the newest commit.
2. Mark superseded findings and resolved bug or compatibility rows in place
   (status column, linked digest); rows are never deleted.
3. Refresh every `Last reconciled` date.
4. Run `python3 tools/docs_lint.py --local`. Paths listed as `local-private` in
   the manifest are never deleted by cleanup.
5. Run `python3 tools/docs_lint.py` strictly and fix every problem.
