# Memory service

Project knowledge lives in one loopback Hindsight 1.2.3 instance, reached only
through `python3 -m tools.memory` (`memory` below) or its `memory` MCP server
([rules](../../.agents/rules/memory.md)). The bank is a deterministic function of
the committed journal `memory/journal/`; this runbook keeps the instance
running, backed up and rebuildable. Every command runs from a checkout of this
repository; the service is shared by all of them.

## Install and start

1. Provision the pinned runtime once per host, and again when
   `tools/hindsight_runtime.requirements.txt` changes (stop the service first):
   `tools/hindsight_runtime install`. In the primary checkout's
   `work/hindsight-runtime/` it installs the Python 3.14 environment from
   hash-pinned wheels, downloads the pinned CPU ONNX models, and extracts the
   PostgreSQL 18.1.0 build bundled in the pinned `pg0` wheel by one throwaway
   start that it stops and drops, then verifies the build against a pinned
   digest. It needs more than 31 GiB free; a missing build makes
   `memory service install` name this command.
2. `memory service install` writes the owner-only connection file
   `connection-v2.env` (key, ports, database password) under
   `$XDG_CONFIG_HOME/icarus-memory/`, the systemd user units
   `icarus-memory.service`, `icarus-memory-backup.service` and
   `icarus-memory-backup.timer`, and starts the service on 127.0.0.1:18681
   (PostgreSQL 18682). Data lives under `$XDG_DATA_HOME/icarus-memory/`. An
   older `connection.env` is moved to the new name with a new key (ports and
   password kept) and the unit restarts; a tombstone named `connection.env`
   stays in its place, on a fresh host too. Never print, log or pass the key,
   and never edit or remove the tombstone.
3. A new instance is empty: `memory rebuild` (needs the 30 GiB free-disk floor),
   then `memory sync` and `memory check --bank`.

Ranking is fused (semantic, keyword, graph, temporal); the cross-encoder
reranker is off by default. `memory service install --reranker N` turns it on
for the whole instance with N candidates and 4 ONNX threads, `--reranker off`
turns it off again; the setting (`ICARUS_MEMORY_RERANKER`) lives in the
connection file and `service status` and `check --bank` assert it. Measured on
the real corpus: recall p50 0.37 s off, 1.7 s at 10 candidates, 12 s at 50.

`memory service start|stop|status|uninstall` manage the unit; `uninstall` keeps
data, connection file and backups. `memory service status` must show every
line `ok`: unit files current, unit active, backup timer active, database
`fsync=on`, anonymous access denied, the feature flags and the bank
configuration of the rules.

## Daily use

Every command replays segments of the current worktree first. After a pull
request merges, the lead runs `memory sync` (fetches `origin/main`, then marks
its revisions accepted). `memory check --bank` compares every document with
the journal and asserts the instance; it changes nothing.

## Backup and restore

- The timer runs `memory backup` daily: a `pg_dump -Fc` with a sha256 sidecar
  and the journal watermark under `$XDG_STATE_HOME/icarus-memory/backups/`,
  7 daily and 4 weekly kept. Run it by hand before risky work.
- `memory restore FILE [--replace]` checks the sidecar and the watermark,
  restores into a fresh data directory, starts the unit, replays this
  checkout's journal forward and removes the previous data directory only
  after status passes. The checkout's `origin/main` must contain the
  watermark's commit (fetch first).
- `memory restore --drill` (milestones): backs up, restores into a scratch
  instance on ports 18683/18684, compares it document by document with the
  live one and with the journal, then removes it.

## Rebuild on a fresh clone

The journal is the off-host copy. On a new host or after losing the data:
clone, `tools/hindsight_runtime install` (Python environment, models and
PostgreSQL, about 2 GB), `memory service install`, `memory rebuild`,
`memory sync`, `memory check --bank`. This is the drill's
second path; it needs no dump. Evidence bytes have no off-host copy:
`memory verify --all` then reports them `unavailable`.

## Worktrees that predate the cut-over

A branch cut before the cut-over still has `docs/ledgers/` and `evidence/`, and
the rows it added there exist nowhere else. If its memory tool is older than
the bank's rendering guard (it contains `8592948e` but not the cut-over), its
scratch guards (`tools/work_gc.py`, `tools/cargo_guard.py`) keep working but
its online `memory` commands print only `connection.env:1: malformed or
repeated line` (the connection file was retired for `connection-v2.env`):
merge `main`, salvaging first, and never repair the config directory. A
worktree older than the memory tool notices nothing until it merges `main`.
`memory check --ci` on `main` reports `this branch predates the cut-over`,
but CI checks the merge commit: do not wait for that message.

Salvage first, merge last: merging `main` first and deleting the legacy files
in its conflicts loses the rows and unprotects the branch's evidence.

1. In the old worktree, commit every legacy row (salvage reads commits). Do
   not merge `main` yet.
2. From an up-to-date checkout of `main`, run on the branch's pre-merge tip
   `memory salvage <branch> --check`, then `memory salvage <branch> --out
   work/<case>/salvage-<n>`, a path that does not exist (`--out must name a
   new directory` otherwise). It writes the proposals under
   `<out>/memory/journal/` and each listed document's body under
   `<out>/documents/<path>`; a non-zero exit for conflicts or held rows leaves
   that output usable. A write failure or an interrupt removes the directory
   when it can and says so when it cannot (`could not remove <out>: …; remove
   it by hand`); an interrupt in the instant after the directory is created
   leaves it empty, and the next run refuses it by name. It reads
   `origin/main` (`--main REF` names another), its REF, CP and RP numbers are
   `origin/main`'s, and a tip that merged the cut-over is refused with the
   pre-merge tip to use. `internal error, nothing written` is a defect in
   salvage: report it.
3. Rows (exit 4 means some row needs a hand):
   - a proposal (`new`, or a revision of `origin/main`'s record): kept;
   - `needs a superseding record: …`: `memory propose` the branch's content
     as a new record, then `memory supersede <old> --by <new>`;
   - `conflict: …: origin/main holds other content under this ID`: when both
     sides allocated the number (as M-1623 to M-1625 on one branch), add the
     branch's record under a new ID and rewrite its citations; when `main`
     took an earlier draft, reconcile with `memory revise` or a superseding
     record;
   - `needs a decision by hand: <ID>: <reason>; not proposed`: the reason is
     one or more clauses, each naming an action by the words it ends with.
     `must be accepted first` (`that record`, `those records` or a named
     record): wait until that record is accepted. `propose it alone first`:
     the row is itself a new record that other rows cite by its provisional
     number; propose it by itself, and once it is accepted the next run
     proposes the rows citing it. `settle the document first`: settle the
     document by the rules below (one that no reference names is settled by
     rule 2). `decide by hand`: decide with the record's owner (a rename,
     records that cannot be told apart, a replay failure, a citation a
     revision cannot change, a document main withdrew or only main changed,
     a record held as a superseding record or a conflict: `its content will be
     recorded under a new ID, and this row's link must name that ID`). Clauses
     come in the order wait, settle, decide, so the reason ends with the
     strongest; do every one, then run salvage again, which reports what is
     left.

   Documents: salvage prints facts, never a command. Apply the first rule
   whose facts fit; `<file>` is the `body written to` path:

   1. `this branch changed it since its base: no` (listed only for `rows held
      because they cite it`, which end `this branch did not change <path>, and
      main has since changed it: decide by hand`): never propose a reference.
      With exactly one reference in force, decide with each row's owner whether
      its claim holds for that reference's current text; with none or several
      there is no single text to judge, so the row stays held, the end state
      unless its owner rewrites the claim as a new record.
   2. `no accepted reference names this path`: if `references that named this
      path and are no longer in force` lists any, or `at this branch's base its
      reference was REF-N: retired` (or `superseded by …`, or `none the
      accepted journal holds`), main withdrew or replaced it: propose nothing;
      rows citing it end `main withdrew the document it cites: decide by hand`
      and stay held unless their owners decide otherwise; bringing the document
      back is the owner's decision, stated in the pull request. Only with that
      history `none` and `none (the document is new on this branch)` is it
      new: propose it as in rule 5, without a supersession; rows citing it are
      proposed once it is accepted.
   3. Several in `in force:`: decide by hand which stands, supersede the
      others if so decided, run salvage again and apply these rules to the new
      facts. Nothing else.
   4. One in force and `main changed or replaced its reference since this
      branch's base: yes`: make `<file>` carry main's current text (`memory get
      REF-N`) with the branch's change, then propose it as a new reference and
      supersede REF-N (rule 5's second branch), never `memory revise`: the
      printed facts describe the branch's text, not the reconciled file, and no
      tool recomputes them. This is deliberately conservative.
   5. One in force, main unchanged: if its `scope tokens` equal what `the
      exporter reads as its scope` and `this text newly names: nothing`,
      `memory revise REF-N --body <file>`. Otherwise a body revision cannot
      change scope: `memory propose reference --set 'title=…' --set-json
      'scope={"basis": "explicit", …}' --note 'salvaged document: <path>'
      --body <file>` with a scope YOU state from what the text supports (the
      tool's lists are what the text names, not a scope) and the note as
      printed, then `memory supersede REF-N --by <the new REF>`. If you judge
      the old scope still covers the newly named tokens, say so in the pull
      request and revise instead: a person records that, never the tool.

   Rows held because they cite a document are proposed on the next run once
   it is done. Example: REF-016, REF-010: 5, revise; REF-008: 5, superseding
   reference; REF-036, REF-001, REF-013: 4, superseding reference.
4. On a new branch from `main`, copy every segment of `<out>/memory/journal/`
   into `memory/journal/`, add the records from step 3, run `memory check
   --ci`, commit and open the pull request. After it merges and the lead runs
   `memory sync`, salvage counts those rows as accepted.
5. Only then merge `main` into the old branch (deleting the legacy files in
   the modify/delete conflicts, citing any new IDs) or retire it. Keep its
   worktree until its evidence records are accepted on `origin/main`: until
   then its legacy shards protect its branch-only artifacts.
6. Confirm from `main`: `memory salvage <branch>` on the merged tip exits 0
   when nothing is left, else 5 naming the pre-merge parent, whose `--check`
   lists what is open and exits non-zero while anything is. A document is
   done once exactly one reference in force names its path (by legacy path
   or note) and holds the branch's body, so a superseding reference counts
   only once the supersession is accepted. Rows resolved by hand and bodies
   merged by hand into other text stay listed: the branch is done when
   everything still listed is one of those and its record, decision or REF
   revision is accepted on `origin/main`. Anything else: back to step 2.

## When the service is down

`memory get --offline ID` and `memory check --ci` read the journal and need no
service; `memory status` prints a journal-only summary (its first line says the
service is unavailable) and a bare `memory check` skips the bank comparison;
`memory verify` and `memory protected-paths` read only the journal and the
evidence bytes. Writes (`propose`, `revise`, `supersede`, `resolve`,
`evidence add`) need the service: wait, or restart it. A command that says
`this tool renders records at version N, older than the bank's M` runs from a
checkout older than the bank: merge `main` into it.

1. `memory service status`; then `memory service start` (it resets the crash
   counter). The unit restarts itself after a crash, at most 10 times in 10
   minutes.
2. Below the 30 GiB floor backups, restores and rebuilds refuse; the service
   still runs. Free space by the storage rules, never by deleting backups or
   evidence.
3. If the data directory is damaged, restore the newest backup; if none is
   usable, rebuild from the journal.
4. `memory check --bank` reporting a document not explained by the journal, a
   stray bank, a mental model, directive, page or webhook means something
   wrote to the instance outside the tool: report it to the owner before
   repairing. A differing document is rewritten from the journal by the next
   command.
