# Native server: run, persistence and inspection

Operating guide for the headless foundation. Design constraints are in
[architecture](../architecture.md); what is proven is in
[implementation status](../implementation-status.md).

## Run

Requires native Linux and Rust 1.99.1 (pinned with Rustfmt and Clippy). Bevy 0.20.0
uses headless features; no GPU, display, original game files or Proton are required.
A compatible server will also need basic data from the free official dedicated
server (Steam app 2089300), fetched unattended at install time (F-828); the game
client is never needed. The install step fetches it anonymously, without
steamcmd or prompts (F-831), into the data directory, where `run` finds it when
`network.data_pak` is unset (`network_listening` reports `data_pak` and its `data_pak_source`, `configured` or `installed`):

```bash
target/release/icarus-server install-official-data --data-dir /var/lib/icarus-native
# -> /var/lib/icarus-native/official/Icarus/Content/Data/data.pak and install.json
```

It prints one JSON record (build, depot, manifest, size, SHA-1 and SHA-256) and
keeps it as `official/install.json`; rerun it after an official update. The
lower-level `icarus-fetch --app 2089300 --file PATH --out DIR` fetches any file
of the official server, and `--list` lists them.

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
data_pak = "/path/to/IcarusServer/Icarus/Content/Data/data.pak"  # optional, official server's data tables
```

The release profile, then the socket, are set up before the data directory
opens; either failure exits nonzero. A Hello with another version gets Upgrade.
Each update polls at most 32 datagrams before the fixed ticks. SIGINT/SIGTERM
stops admission, closes every session, then drains and saves. `network_listening`
gives the bound address and `version_check`; `status` and `stopped` carry
`sessions` counts only. A world seeded by `preserve-save` sends its save's
active prospect and game mode values in the GameState open
(`active_prospect_replicated`, F-804); with `data_pak` (the free official
server's `Icarus/Content/Data/data.pak`, F-828) its saved session flags also
replicate as row indices, and the record counts the flags the tables lack
(`unmapped_session_flags`, F-830) and the missions sent as `LockedMissions`
(`locked_missions`, F-835). With the tables every pawn opens with a fresh
player's vitals and empty inventories from them (F-836). No ICARUS client compatibility,
identity check, replication or gameplay is implied; only the authored probe
is tested:

```bash
cargo test --locked --test network
target/release/icarus-server run --config server.toml --data-dir work/net-smoke --max-ticks 600
cargo run --locked --example session_churn -- --server 127.0.0.1:17777 \
  --cycles 200 --concurrency 1 --release-profile config/release.toml
# Hold joined sessions and check their GameState updates (F-833);
# --sessions N (default 1) holds N at once and prints the aggregate summary.
cargo run --locked --release --example session_hold -- --server 127.0.0.1:17777 \
  --release-profile config/release.toml --seconds 3600 --sessions 8
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
  role against the native catalog, blockers; F-767, F-821). Without `--dry-run`
  the import is refused; nothing is written. `--catalog FILE` resolves
  references against a native content catalog: a regular TOML file of at most
  4 MiB with `format = 1` and, under `[references]`, one list per role
  (`actor_class`, `actor_path`, `data_row`, `asset_path`, `world_tile`,
  `class_variable`, `unclassified`); a reference maps only in the roles that
  list it. Instance names are never resolved and have no list.
  `--data-pak PATH` also resolves data-row references against every table of
  the official server's data.pak, without case (F-840).

Value meanings and original-server round trips remain unvalidated. The
[synthetic workload](synthetic-workload.md) exercises native scheduling and
durability; it is not an ICARUS performance benchmark.
