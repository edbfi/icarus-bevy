# Documentation rules

Documentation has two surfaces. `tools/docs_lint.py` enforces the mechanical
rules through prek and CI; the rest are reviewed. A document that says less but
is readable beats one that records everything: cite an ID instead of restating
a claim.

## Surfaces

- Current surface: `README.md` and everything under `docs/` except the frozen
  directories. Bounded, rewritten in place, lint-enforced.
- Retained history: `docs/history/` and `docs/research/archive/`. Verbatim,
  append-only, frozen to the lint allowlist. Only a banner and link targets may
  change. New files there require a deliberate allowlist edit.

## Where things go

| Content | Location |
| --- | --- |
| Goal, milestones, exit criteria, release targets | `docs/linux-dedicated-server-plan.md` |
| Summary, milestone state, next tasks (at most ten) | `docs/implementation-status.md` |
| Preconditions that block actions, grouped by action (only home) | `docs/gates.md` |
| Design constraints, repository boundaries, toolchain | `docs/architecture.md` |
| Operating procedures and commands | `docs/runbooks/` |
| Durable format and protocol facts | `docs/specs/<area>/<topic>.md` (fixed H2 set) |
| Findings by area; retired rows | `docs/ledgers/findings/<area>.md`; `docs/ledgers/findings/retired/` |
| Open questions, bugs, compatibility | `docs/ledgers/unknowns.md`, `bugs.md`, `compatibility.md` |
| Research digest per question cluster | `docs/research/<area>/<topic>.md` (fixed H2 set) |
| Evidence index: one record per artifact (path, digest, class, kind, status, cited REA IDs) | `evidence/<area>[/<topic>]/NNN.jsonl`, the folder named after its findings ledger; schema in `evidence/README.md` |
| Exported REA bundles, session ledgers and scratch | `work/` (never tracked; `.agents/rules/work.md`) |
| Historical authorizations and reports | `docs/history/`, `docs/research/archive/` |
| How agents work in this repository | `.agents/rules/` only |

Specs use Status, Scope, Confirmed, Candidate, Open questions, Evidence.
Digests use Question cluster, Current answer (sentences citing F- and U- IDs),
Open, Artifacts (one line of M- IDs). Status uses Summary, Milestones, Next
tasks. Finding columns: ID, Claim, Kind, Evidence (evidence class; IDs; source
revision), Scope and limits (confidence; boundary; limitations), Unknowns,
Status.

## Read tiers

| Tier | Contents | Budget |
| --- | --- | --- |
| Startup | `CLAUDE.md`, `docs/README.md`, `docs/implementation-status.md` | 20 KB, lint fails over |
| Task entry | one rules file, `gates.md`, the area digest, one spec, the task's skill (routing table in `CLAUDE.md`) | 64 KB, lint reports |
| Retrieval | one F- or U- row by `grep -w ID -r docs/ledgers`, one M- row by `grep -rhw --include='*.jsonl' ID evidence/`; never a whole ledger or shard | per row |

## Rules

1. Admission. A finding is a claim about the original artifact, format or
   protocol; a measured native-server property; or a conclusion that a spec,
   gate, decision or the avoidance of repeated work depends on, including
   negative results. Classify by substance. Receipts (ran, compiled, reviewed,
   cleaned up, paused) are evidence index rows: lint rejects new finding rows
   whose evidence starts with `tool-receipt` or `tooling-receipt`. Each REA
   session's exported bundle (`work/<case>/rea-bundle.json`) is the canonical
   record of what it observed; its `rea-bundle` row lists the Evidence and
   unknown IDs that rows cite as `M-NNN/ev_<12 hex>` or `M-NNN/unk_<12 hex>`.
   Full `ev_`/`unk_` identifiers never appear in documents, and pre-REA case
   notes are cited as `M-NNN/E-NNN`.
2. Session close edits only documents whose authoritative content changed: the
   area digest; a spec if a durable fact changed; finding rows added or
   superseded; `unknowns.md`; `gates.md`; evidence index rows (one `rea-bundle`
   row per REA session, one receipt row per bounded run; a new row names one
   digested file, never a directory; reuse a row before adding one). In status, only
   Next-tasks items may be marked done or removed. Create a digest only for a
   new question cluster.
3. Consolidate before adding. When a new row would extend a chain of
   incremental rows, write or update the chain's summary row, carrying the
   chain's evidence IDs, and mark the steps `superseded-by F-NNN`.
4. Drop policy. Near a cap, split by topic (specs, digests, active findings,
   numbered retired files such as `retired/<area>-2.md`) or supersede and retire
   finding rows. Retire rows that are superseded, historical session state or
   of no continuing relevance; being uncited is a review signal, never a
   criterion, and negative results stay active. Deleting prose that restates an
   ID-retrievable claim is permitted and preferred; rewording a row to fit its
   cap is permitted. Frozen directories are never a destination. Removing
   whitespace to fit a cap is a lint failure. Index rows are retired in place
   (`status: retired` with `superseded_by` or `note`); a `local-private`
   artifact may be deleted only after its row is retired, and `missing` records
   an artifact that can no longer be located.
5. Checkpoint reconcile when status `Last reconciled` is older than 30 days or
   the lint staleness warning fires: reconcile status against HEAD, consolidate,
   retire and refresh dates.
6. Current-surface files are rewritten in place: no dated or timestamped
   headings or filenames, no appended session notes, no session narrative, tool
   output or restated background. Prose cites F-, U- and M- IDs instead of
   hashes, clock times or `work/` paths; code spans, fenced blocks, runbooks,
   gates and ledger cells are exempt. A rewritten file keeps exactly one
   `Before restructure: <sha>:<old path>` provenance line.
7. IDs are permanent and form one space per prefix. Never delete or renumber a
   row; retire it with its full text. A pushed M- row changes only its
   `status`, `superseded_by`, `note`, `label` and, for legacy rows, `kind`; a
   corrected `path` or `sha256` keeps the old value in `note`. Every status, spec and ledger file carries
   `Last reconciled: YYYY-MM-DD`. Canonical workflow rules live under
   `.agents/rules/`; `CLAUDE.md` (`AGENTS.md`) summarizes and links to them.

## Caps

Bytes: 16 KB per current-surface file, 8 KB per digest; active findings 48 KB
per file and 96 KB in total, retired findings 64 KB per file; unknowns, bugs and
compatibility 64 KB each; 400 bytes per ledger row; evidence shards 64 KB with
numbered rollover per folder, records 2 KB, labels 160 characters; 2 KB for
`evidence/README.md`; 160 KB for the current prose surface (every current file
except the ledgers).

## Lint

`python3 tools/docs_lint.py` checks caps, heading sets, links and anchors,
document paths named in code, `CLAUDE.md` and `.agents/rules/`, ID definition
and uniqueness across `docs/ledgers/findings/`, IDs removed since `--base`
(default `HEAD`), new receipt findings, evidence detail in prose, glued tokens,
dated filenames, provenance lines, read tiers, evidence records and their
citations. `--local` also verifies active `local-private` paths, digests,
bundle membership of listed IDs and every file a `fixity.sha256` row lists;
`--next-id` allocates IDs and `--list-local` lists paths to back up, including
those files. Paths of active `local-private` rows are never deleted
by cleanup.

## Milestone checklist

1. Reconcile `docs/implementation-status.md` against the newest commit.
2. Consolidate and retire findings; mark resolved bug and compatibility rows;
   retire index rows whose artifacts were consolidated or cleaned.
3. Refresh every `Last reconciled` date.
4. Run `python3 tools/docs_lint.py --local`, then fix every problem the strict
   run reports.
5. Prune `work/` and review its signals (`.agents/rules/work.md` routine 2).
