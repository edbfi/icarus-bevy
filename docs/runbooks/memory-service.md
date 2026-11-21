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
the rows it added there exist nowhere else. A worktree whose memory tool is
older than the bank's rendering guard (it contains `8592948e` but not the
cut-over) parses `main`'s journal, and its scratch guards (`tools/work_gc.py`,
`tools/cargo_guard.py`) keep working, but its online `memory` commands stop at
`connection.env:1: malformed or repeated line`, because the service's
connection file was retired for `connection-v2.env`. That error is all it
prints. It means: merge `main` (salvaging first, below), never repair the
config directory. A worktree older than the memory tool keeps its own
legacy hooks and notices nothing until it merges `main`. On `main`,
`memory check --ci` on such a branch says `this branch predates the cut-over`
and lists its rows, but its own hooks are old and CI checks the merge commit,
so do not wait for that message.

Salvage first, merge last. Merging `main` first and resolving the
modify/delete conflicts by deletion loses the rows, and the branch's evidence
stops being protected as soon as its `evidence/` leaves the working tree.

1. In the old worktree, commit every legacy row it holds (salvage reads
   commits, not the working tree). Do not merge `main` yet.
2. From an up-to-date checkout of `main` in the same repository, list and
   convert the rows of the branch's pre-merge tip:
   `memory salvage <branch> --check`, then
   `memory salvage <branch> --out work/<case>/salvage-<n>`. `--out` names a
   directory that does not exist yet (`<out> exists: --out must name a new
   directory` otherwise); salvage creates it, writes the proposals under
   `<out>/memory/journal/` and each pending document's body under
   `<out>/documents/<path>`, and removes it if the run fails. Salvage
   resolves `origin/main` (`--main REF` names another); REF, CP and RP
   numbers in the report are `origin/main`'s. A tip that already merged the
   cut-over is refused with the pre-merge tip to use.
   `memory salvage: internal error, nothing written: …` is a defect in salvage
   itself: report it and do not work around it.
3. Act on each line of the report (exit 4 means some row needs a hand):
   - a proposal (`new`, or a revision of `origin/main`'s record): kept in the
     output segments;
   - `needs a superseding record: …`: `memory propose` the branch's content as
     a new record, then `memory supersede <old> --by <new>`;
   - `needs a decision by hand: …`: settle it with the record's owner. One
     that says `it cites the new document <path> (REF-N, a provisional
     number), whose record must be accepted first` (or a new CP/RP row) waits:
     get that record accepted, then run salvage again, which then proposes
     the row with the accepted ID. Rows that say `one of N records sharing
     the …`, `which the accepted journal holds N times` or `a rename needs a
     superseding record; decide by hand` are settled by hand, as is any row
     citing one;
   - `conflict: …: origin/main holds other content under this ID`: when both
     sides allocated the number (as M-1623 to M-1625 on one existing branch), add the
     branch's record under a new ID (`memory evidence add`, `memory propose`)
     and rewrite the branch's citations; when `main` took an earlier draft,
     reconcile by hand with `memory revise` or a superseding record;
   - a pending document: `--out` prints the commands that use the body it
     wrote. A changed document whose text names the same builds, releases and
     commits as REF-N's scope: `memory revise REF-N --body
     <out>/documents/<path>`, run as printed. One whose text names others
     (`changed document; its text names: …, REF-N's scope is …: a body
     revision cannot change scope; propose it as a new reference and
     supersede REF-N`) and a new document: `memory propose reference --set
     'title=…' --set-json 'scope=<STATE THE SCOPE>' --note 'salvaged document:
     <path>' --body <out>/documents/<path>`, refused until you replace
     `<STATE THE SCOPE>` with a JSON scope you state (the line `the scope must
     be stated by whoever proposes it: … the text names: …` lists the text's
     tokens, which salvage neither keeps nor upgrades); for the changed one,
     then `memory supersede REF-N --by <the new REF>`. Keep the note: it is how
     salvage recognises that REF; without it the document and the rows citing
     it stay listed. A path `main` holds twice (`the accepted journal holds
     REF-A, REF-B for this path: decide by hand (no command)`) is settled by
     hand.
4. On a new branch from `main`, copy every segment of `<out>/memory/journal/`
   into `memory/journal/`, add the records from step 3, run
   `memory check --ci`, commit and open the pull request. After it merges, the
   lead runs `memory sync`; salvage then counts those rows as accepted, never as
   conflicts.
5. Only then merge `main` into the old branch (deleting the legacy files in
   the modify/delete conflicts and citing any new IDs), or retire the branch.
   Keep its worktree until its evidence records are accepted on `origin/main`:
   until then its legacy shards are what protects its branch-only artifacts.
6. Confirm it is done, from the up-to-date checkout of `main`:
   `memory salvage <branch>` on the merged tip exits 0 when nothing is left.
   While anything is left it exits 5 and names the pre-merge parent;
   `memory salvage <parent> --check` lists what is left, the rows and the
   pending documents (each with the REF to revise), and exits non-zero while
   either remains. A document is done once an accepted REF of its path (by
   legacy path, or by the `salvaged document: <path>` note) holds the
   branch's body; salvage does not check the `memory supersede` of a
   superseding REF, so confirm `memory get REF-N` shows it superseded. A row
   resolved by hand in step 3 (a superseding record, a conflict added again
   under a new ID, a decision by hand) never becomes accepted under its old
   ID, and a body merged by hand into other text never equals the file, so
   both stay listed and the exit stays non-zero. That is expected for those
   only: the branch is done when everything still listed is one you resolved
   by hand and its superseding record, new record, recorded decision or REF
   revision is accepted on `origin/main`. Anything else still listed is not
   salvaged yet: go back to step 2.

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
