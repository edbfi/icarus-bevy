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
   `tools/hindsight_runtime install`. It shares a Python 3.14 environment,
   the pg0 PostgreSQL 18.1.0 build and pinned CPU ONNX models in the primary
   checkout's `work/hindsight-runtime/`, and needs more than 31 GiB free.
2. `memory service install` writes the owner-only connection file (key, ports,
   database password) under `$XDG_CONFIG_HOME/icarus-memory/`, the systemd user
   units `icarus-memory.service`, `icarus-memory-backup.service` and
   `icarus-memory-backup.timer`, and starts the service on 127.0.0.1:18681
   (PostgreSQL 18682). Data lives under `$XDG_DATA_HOME/icarus-memory/`.
   Never print, log or pass the key.
3. A new instance is empty: `memory rebuild` (needs the 30 GiB free-disk floor),
   then `memory sync` and `memory check --bank`.

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
clone, `tools/hindsight_runtime install`, `memory service install`,
`memory rebuild`, `memory sync`, `memory check --bank`. This is the drill's
second path; it needs no dump. Evidence bytes have no off-host copy:
`memory verify --all` then reports them `unavailable`.

## When the service is down

`memory get --offline ID` and `memory check --ci` read the journal and need no
service; `memory verify` and `memory protected-paths` read only the journal and
the evidence bytes. Writes (`propose`, `revise`, `supersede`, `resolve`,
`evidence add`) need the service: wait, or restart it.

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
