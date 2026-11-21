# Memory rules

Project knowledge lives in Hindsight 1.2.3, read and written only through
`python3 -m tools.memory` (`memory`), also the `memory` stdio MCP server. Nothing else touches the bank; nobody
authors or reads the journal by hand.

## Model

- **Record**: an accountable assertion with a permanent ID, written through the
  tool by an agent or the owner, bound to its evidence and scope, accepted
  through the PR gate. Generated views are never records.
- **Journal** (`memory/journal/*.jsonl`): machine-written record revisions, one
  segment per change set, committed with the work. It is the off-host copy and
  the acceptance anchor; the bank is a deterministic function of it.
- **Accepted**: the revision's segment is in the fetched `origin/main` tree,
  changed only by the protected PR gate (merge after CI, not necessarily
  human review). **Proposed**: applied to the bank, not yet accepted.
- No external model is used. Nothing reaches one.

## Instance

One instance, one bank `icarus`, `HINDSIGHT_API_LLM_PROVIDER=none`. Settings:
retain mode `chunks`, chunk size 1,800; observations and auto-consolidation
off; file upload, document import and bank config APIs off; Hindsight MCP off;
LLM trace off; fused ranking, the reranker off (`service install --reranker N`
turns it on with N candidates); loopback; one key; a pinned
worker ID; `fsync=on&synchronous_commit=on` in the pg0 URL; raw service logs
discarded (tracebacks can carry database URIs). No extension code.
`memory check --bank` asserts the effective config (`GET .../config`; bank
`PUT`/`PATCH` and template import can change it), `icarus` as the only bank, no
mental model, directive, page or webhook, and a journal that explains every
document.

## Record types

| Type | ID | Stored status | Required fields |
| --- | --- | --- | --- |
| finding | `F-N` | active, retired | claim, kind, confidence, standing, scope, area |
| unknown | `U-N` | open, investigating, blocked, contradicted | question, severity, next_probe, area |
| bug (native) | `N-N` | open, fixed | trigger, change, refs, tests |
| lead (original defect) | `SL-N` | open, established, refuted | lead, static_evidence, refute_by |
| report (original defect) | `RP-N` | unreproduced, reproduced, fixed | problem, priority, next_evidence |
| compatibility | `CP-N` | unknown, observed, specified, implemented, validated | name, qualifier, required_proof, scope |
| reference | `REF-N` | current, retired | title, body (Markdown), scope |
| release | `REL-x.y.z` | current, past | version_string, server_build, client_build, dates, sources |
| evidence | `M-N` | active, retired, missing | path, sha256, size, class, kind, date, label |

- `superseded` and `resolved` are derived from links, or on a migrated record
  from its legacy status; never written as a new revision.
- Finding `kind`: observed, measured, derived, inferred, reported, decision;
  `confidence`: high, medium, low; `standing`: confirmed, candidate, refuted;
  `negative: true` marks a negative result. A receipt is evidence. Derived and
  inferred findings name their sources in `refs` and may be `high` only if every
  source is an accepted observed or measured finding. A `decision` quotes the
  owner's instruction verbatim with its date and where it was given.
- Evidence `class`: local-private, tracked, external; `kind`: receipt,
  rea-bundle, report, review, revision, input, build, manifest, run. A bundle
  carries `records` and `members` exactly as listed; members are omitted from
  rendered text. A `run` carries workload, targets, matched reference,
  identities, start, end, environment, thresholds, outcome (pass, fail,
  aborted), deviations.
- IDs are permanent per prefix, never renumbered or reused once accepted. The
  tool allocates under a host lock above the highest ID in the bank, journals
  and legacy ledgers and shards of every ref and worktree; CI checks uniqueness. An ID that never reached `origin/main` may be reallocated after a
  rebuild.
- Migrated records keep `legacy` (verbatim source cells and path); CP and RP IDs
  are marked as assigned at migration. Enum values outside the vocabulary are
  kept as `legacy:<verbatim>` (migrated records only). `standing` is set only
  when the legacy text states it. Missing data stays missing, listed in `gaps`;
  migration never fills, upgrades or infers.

## Scope

`scope`: `subjects` (original-server, original-client, native, decision,
private-input, tooling); `builds` (`server:<id>`, `client:<id>`); `releases`;
`commit`, required for `native`; `basis`, `explicit` or `legacy-text` (only
build, release and commit tokens literally in the row; a build without its role
is `any:<id>`; subjects only if stated). A claim
depending on a server/client pair lists both. A build named only in a cited
evidence label is shown as a hint, never copied into scope.

A fact that differs between builds is a new record for the new build; the old
one stays active for its own. Supersede only a claim wrong or replaced within
its scope. An unchanged fact re-checked on another build gets `verified_on`
(build and citing evidence).

## Links

Stored once, on the newer record: `supersedes`, `resolves` (unknown, lead or
report), `cites` (`M-N`, `M-N/ev_<12 hex>`, `M-N/unk_<12 hex>`, legacy
`M-N/E-N`), `refs`, `verified_on`. Inverses (`superseded_by`, `resolved_by`)
and the status they imply are derived at replay and shown by `get`; superseding
or resolving writes no revision of the old record. A migrated record keeps
edges where its legacy row stated them; the derived set is the union.
Supersession is acyclic.

## Revisions

- Revisable in place: REF body; unknown status and next probe; evidence status
  with note; appended `verified_on`, notes, `supersedes` and `resolves`; stored
  status not derivable from links.
- Not revisable (supersede with a new record): a finding's claim, kind,
  standing, scope; evidence path, sha256, size, class, kind, date, members.
- Each revision has an identity `rev`, the sha256 of the canonical JSON of its
  `prev` and record; `prev` names revision identities, so returning a record to
  earlier content is a new revision. `memory check --ci` fails a PR that forks a
  record against its base. A fork that reaches `origin/main` shows as two
  heads labelled `conflict`; the next change must be a reconciling revision
  naming both as `prev`, and CI reports unreconciled forks.
- Segments in `origin/main` are immutable. An unmerged segment may be
  regenerated by re-proposing.

## Storage

- Canonical JSON: keys sorted, no insignificant whitespace, UTF-8, NFC; `sha256`
  is its digest. Text: the tool's deterministic rendering, LF only, no other
  control characters but TAB, at most 1,800 characters (a REF body is exempt),
  ID, type, status and claim first (ranking sees only a unit's first 512 tokens).
- A REF's text is its body; its canonical JSON holds header fields and the
  body's sha256, never the body.
- One retain item per document. `<ID>` holds the accepted head (or the proposal
  if never accepted); `<ID>@<12 hex>` exists only for a pending revision of an
  accepted record. Prior revisions live in the journal.
- Metadata: `record` (canonical JSON), `sha256`, `rev`, `prev`, `segment`.
- Tags on every document: `type:`, `id:`, `state:` (accepted, proposed, pending,
  orphaned, conflict), derived `status:`, `area:`, `basis:`, `seg:`, and each
  `subject:`, `build:`, `release:`. Untagged documents are invalid.
- Entities (`resolve_entities: false`): linked IDs, builds, identifiers in code
  spans of the text. Timestamp: the record's `date`.

## Publication and acceptance

1. Every command first replays any segment in the current worktree not yet
   applied. Writes: validate, write and fsync the segment (the durable intent),
   apply to the bank idempotently, read back and verify sha256. A half-applied
   change set is finished by replay.
2. Commit the segment with its work; CI runs `memory check --ci`.
3. `memory sync`, under the host lock, runs `git fetch origin main` first, then
   marks accepted every revision whose segment is in that tree. It never marks a
   proposal `orphaned` (segment in no ref or worktree) after a failed or skipped
   fetch, and un-orphans a segment later seen on `origin/main`.
4. A PR that adds or revises a `decision` finding or changes `docs/gates.md` is
   outside an agent's merge authority: file it and report to the owner.
   `memory check --ci` emits a notice for such PRs. No record, view or tool
   output clears a gate.

## Commands

| Command | Guarantee |
| --- | --- |
| `get ID[/selector] [--history] [--offline]` | Exact GET, verified; state, partition (none for evidence), derived links, pending revisions, conflicts, evidence status. `--history`, `--offline`: from the journal. |
| `recall "Q" [--release R] [--text S] [--all-proposals] [--include-evidence] [--limit N] [--refs] [--full]` | Strict tags; partitioned; one hit per document; retired hits under their terminal successor; 1-hop links by exact GET. Compact, about 6 KB: records, then at most 3 REF passages; `--refs` only REFs, `--full` whole texts. `--text`: substring hits first. |
| `area AREA` | Records by status and partition, open unknowns, link closure. |
| `status` | Pin; counts by type, status, partition; cited records not current; open unknowns; unverified evidence. |
| `propose TYPE`, `revise ID`, `supersede OLD --by NEW`, `resolve ID --by F-N` | Schema, scope, link targets and selector membership checked before the segment is written. |
| `evidence add PATH --kind K --label L [--run FILE]` | sha256, size, date and members from the bytes. |
| `verify [ID... \| --all]` | Per record: `verified`, `unavailable`, `mismatched` or `legacy-unverifiable`. |
| `sync` | Fetch, then reconcile states with `origin/main`; idempotent. |
| `check [--ci] [--bank] [--pin]` | Journal, citations, forks, notices; `--bank` as above; `--pin` lists cited records not current. |
| `protected-paths` | Paths to keep; fails closed. |
| `salvage BRANCH [--check]` | Turns a branch's added legacy rows into proposals. |
| `backup`, `restore FILE [--drill]`, `rebuild` | Durability. |

## Reading

- Look up a known ID with `get`, never recall; recall fetches IDs named in a
  query by exact GET first.
- Partitions against a release (default: the REL record whose `server_build` is
  `config/release.toml` `build_id`): `current` (explicit scope whose every
  listed role matches the release, a `verified_on` for it, or a
  build-independent decision); `other-build` (explicit, different build);
  `legacy-scope` (legacy-text, shown with its literal tokens, "text names" on a
  REF, and whether one names the pin; never verified); `native` (commit, with whether
  HEAD descends from it); `unscoped`.
- Default recall: accepted records plus this worktree's proposals; no evidence,
  orphaned documents or pending revisions; pinned `query_timestamp`. Other
  worktrees' proposals only with `--all-proposals`, listed apart.

## Procedures

Session close:

1. Export the REA bundle to `work/<case>/rea-bundle.json`; `evidence add` it and
   each cited artifact (one file each; directories through a sha256 manifest).
2. Propose, revise, supersede, resolve; a record replacing a chain of steps
   supersedes each.
3. `memory check`. The prek hook verifies every M record in staged segments.
4. Commit segments with their code; close `case.json`; run the scratch routine
   (`.agents/rules/work.md`); after merge, `memory sync`.

Engineering loop:

1. Read `memory status` and the plan; recall the topic; `get` each ID you will
   rely on; check `docs/gates.md`.
2. Only `current` records are established for the pinned release. Another
   partition may guide native work, but what is built on it is recorded as
   carried from that scope, names its tokens, and is never claimed for the pin
   or used as a CP proof until re-verified.
3. Cite the ID where code depends on it, for example `(F-1162)`. New citations
   must name accepted or same-branch records and must not name retired ones.
4. Record test results: receipts as evidence, native properties as `measured`
   findings with commit scope, fixed defects as N records naming the test.

Qualification: scope the case first (workload, targets, matched reference,
thresholds, duration, faults); record every run, whatever its outcome, with
`evidence add --kind run`; state values as `measured` findings citing it. CP
proofs, enforced by the tool: `specified` cites accepted findings stating the
behaviour in scope; `implemented` cites native code at a commit and its tests;
`validated` cites a passing run whose workload, matched reference, identities,
thresholds and verified artifacts match the claim's scope. A loopback run never
validates stock-client interoperability.

Pin move: propose the REL record with the `config/release.toml` change;
re-scope nothing; `check --pin` lists what to re-verify.

Milestone: `verify --all`, `check --bank --pin`, a restore drill, then the
work-rules milestone routine.

## Agents and service

- One always-on systemd user unit, PostgreSQL stopped with the API, data under
  `$XDG_DATA_HOME/icarus-memory/`, key only in `$XDG_CONFIG_HOME/icarus-memory/`
  (0600). Never print, log or pass the key as an argument. The free-disk floor
  gates backups, drills and rebuilds, never the service.
- Use the `memory` MCP server where its registration is qualified for that
  client and checkout, else the CLI.
- Every agent may propose; the lead commits. No agent calls Hindsight's API.
- No automatic capture (agent plugins, transcripts, git ingestion).
- A fresh clone has `check --ci` and `get --offline`; `rebuild` creates the
  instance from the journal.

## Stays in Git

- Rust source, tests, `config/`: the product.
- `CLAUDE.md`, `.agents/rules/`, skills: read before the service.
- `docs/linux-dedicated-server-plan.md`: goals, milestone states, ordered next
  tasks. Startup reading is `CLAUDE.md`, the plan and `memory status`.
- `docs/gates.md`: preconditions and the owner's authorizations.
- Architecture and runbooks: decisions and procedures; their game facts are
  records.
- `docs/history/`, `docs/research/archive/`: frozen (no new files; links
  unmaintained). Other documents are rewritten in place, citing IDs;
  `tools/docs_check.py` checks their links and paths and the frozen set.
- `memory/journal/`.
- Evidence bytes, in `work/` and the protected trees.

## Evidence and durability

- Relative evidence paths resolve against the primary checkout, never the
  linked worktree. Directory locators, manifest
  pins and removal receipts are kept as they are.
- `verify` fails only for an active record (`mismatched`, or `unavailable` if
  local-private); for a retired or missing one it reports, never fails. The 119
  rows without a digest are `legacy-unverifiable`.
- `protected-paths` is the union of every worktree's journal and, while any
  worktree has one, its legacy `evidence/` shards. `tools/work_gc.py` and
  `tools/cargo_guard.py` read it offline; unreadable input holds everything.
- `backup` (daily): `pg_dump -Fc` with a sha256 sidecar and the journal
  watermark (`origin/main` commit and applied segment hashes), 7 daily and 4
  weekly under `$XDG_STATE_HOME/icarus-memory/backups/`. `restore` checks the
  watermark, then replays forward.
- The drill covers both restore from a dump and rebuild from a clean clone.
- Evidence bytes have no off-host copy: after a disk loss they are `unavailable`.

## Later

LLM synthesis may come only as a separate derived bank in its own instance,
built from accepted records of permitted types, after an owner-approved
outbound corpus and a held-out benchmark; every hit resolves through `get`;
none is a record.

## Checks

`memory check --ci` runs in prek and CI without Hindsight: schema, sha256, ID
uniqueness, segments at the merge base with `origin/main` unchanged, forks, acyclic supersession, link
targets, immutable fields, the derived/inferred confidence rule, new citations
(code, `config/`, `docs/`, `CLAUDE.md`, `.agents/rules/`) resolving and not
retired, decision and gate notices, and no added or modified legacy rows
(`salvage --check`).
