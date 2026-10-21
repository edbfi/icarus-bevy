# Native server: run, persistence and inspection

Operating guide for the headless foundation. Design constraints are in
[architecture](../architecture.md); what is proven is in
[implementation status](../implementation-status.md).

## Run

Requires native Linux and Rust 1.99.1 (pinned with Rustfmt and Clippy). Bevy 0.20.0
uses headless features; no GPU, display, original game files or Proton are required.

```bash
cargo build --release --locked
cargo run --locked --bin icarus-server -- check-config --config config/server.example.toml
cargo run --release --locked --bin icarus-server -- run --config config/server.example.toml
```

Use SIGINT or SIGTERM to stop and save. For a bounded smoke run:

```bash
target/release/icarus-server run --data-dir work/smoke-world --max-ticks 10
```

For qualification, `--compressed` (requires `--max-ticks`) runs those ticks
as simulated time without pacing: autosave and status intervals count
simulated seconds, and a due autosave awaits one still in flight, so a long
schedule of saves runs in minutes ([synthetic workload](synthetic-workload.md)).

The development default is 30 Hz with at most four catch-up ticks per update.
This is provisional, not a discovered ICARUS tick rate. No game/query sockets
open unless the experimental `[network]` section below is enabled.
The process emits JSON lifecycle/status/save records to stderr and always reports
`gameplay_ready: false`. Tick timing currently measures only foundation systems,
not representative gameplay performance.

Configuration precedence is CLI → TOML → defaults. Unknown fields and invalid
limits are rejected. Relative paths resolve from the working directory.
Config inputs must resolve to an opened regular file of at most 64 KiB; regular
symlinks are supported. FIFO and oversized inputs fail before runtime state opens
(F-671).
`check-config` prints effective configuration without creating the data directory.
Use `--help` on each subcommand for supported overrides.

## Network sessions (experimental)

An opt-in `[network]` table (disabled by default) runs the loopback peer lab
inside `run` (F-768, F-770):

```toml
[network]
enabled = true
bind = "127.0.0.1"          # IPv4 loopback only
port = 17777                # 0 selects an ephemeral port; CLI: --network-port
max_players = 8             # admission occupancy limit, 1..=64
session_timeout_seconds = 30  # idle eviction, 1..=600, monotonic real time
release_profile = "config/release.toml"  # optional Hello version pin
```

The release profile, then the socket, are set up before the data directory
opens; either failure exits nonzero. A Hello with another version gets Upgrade.
Each update polls at most 32 datagrams before the fixed ticks. SIGINT/SIGTERM
stops admission, closes every session, then drains and saves. `network_listening`
gives the bound address and `version_check`; `status` and `stopped` carry
`sessions` counts only. A world seeded by `preserve-save` sends its save's
active prospect in the GameState open (`active_prospect_replicated`, F-802). No ICARUS client compatibility, identity check,
replication or gameplay is implied; only the authored probe is tested:

```bash
cargo test --locked --test network
target/release/icarus-server run --config server.toml --data-dir work/net-smoke --max-ticks 600
cargo run --locked --example session_churn -- --server 127.0.0.1:17777 \
  --cycles 200 --concurrency 1 --release-profile config/release.toml
```

## Replication scheduling (experimental)

`replication::ReplicationPlugin` runs in `FixedUpdate` between gameplay and
tick finish. Per connection (`NetViewer`) it builds the consider list, tests
relevancy, ranks priority and keeps an authored per-connection budget by
calling the `experimental::actor_channel` evaluators; `ReplicationFrame` holds
domain-ID results and stage timings. Net parameters are UE engine defaults,
not ICARUS values. An optional grid (`SpatialMode::Grid`, off by default)
gives exactly the naive relevant set. It sends nothing and models no
bandwidth, dormancy, ownership or destruction.

```bash
cargo run --release --locked --example replication_bench -- --viewers 1,4,8
# --source <envelope>: planned-actor positions; prints aggregates only
```

## Persistence and recovery

Default state location: ignored `work/runtime/`, one writer per directory
(advisory lock), autosave every 60 seconds through a bounded worker. Saves hold
authored domain records and a simulation clock, not ICARUS gameplay. Capture
fails before submission if an entity with a domain ID lacks its position
(F-482). Schema 1 holds records and the clock (64 MiB per generation); a world
with a preserved prospect saves schema 3, referencing a write-once,
digest-verified `preserved-<sha256>.bin` sidecar; schema 2 still loads, other
schemas fail closed (F-567).

Each generation is checksummed and synchronized before the manifest is
atomically published; two committed generations are kept, and a corrupt
current one recovers from the previous (F-228, F-771). Corrupt or missing
manifests beside snapshots, noncanonical digests and unsupported schemas fail
closed instead of creating a fresh world (F-417, F-428). I/O errors other than
a missing current file stop loading; repair the fault and retry (F-452).
Preserve a copy before manual recovery.

SIGINT/SIGTERM freezes simulation, drains outstanding work and saves a final
snapshot within a deadline, exiting nonzero on failure; it is not a hard
process-exit-time guarantee (F-230, F-672). Durability is qualified for local
Linux storage, not network filesystems or physical power loss (U-035).

## Supplied saves

All modes open the source nonblocking and admit only a regular file (regular
symlinks allowed; F-671), read it, and write nothing unless stated:

- `inspect-save --source PATH`: bounded [envelope checks](../specs/save-format/prospect-envelope.md);
  `--inner-tags` adds [root framing](../specs/save-format/tagged-properties.md),
  `--recorders` recorder framing, counts only.
- `--stage-check [--expect-prospect-name NAME]`: full `FProspectSaveState`
  staging, migration, `ProspectInfo` and every recorder's base component (F-738).
- `--typed-check`: staging, then every recorder value typed and re-encoded
  exactly; gaps counted by reason per class pseudonym (F-766).
- `--preserve-check`: lossless preservation saved and reloaded through a
  temporary schema-3 store it removes, with sizes, timings and memory (F-567, F-584).
- `preserve-save --source … --data-dir …`: seeds a new world holding only the
  preserved prospect (tick 0); refuses an existing world.
- `import-save --source … --destination … --dry-run`: prints the import plan
  (staging, `Validate`, typing, base fields, transforms, content references by
  role against the native catalog, blockers; F-767, F-769). Without `--dry-run`
  the import is refused; nothing is written.

Value meanings and original-server round trips remain unvalidated. The
[synthetic workload](synthetic-workload.md) exercises native scheduling and
durability; it is not an ICARUS performance benchmark.
