# Native server: run, persistence and inspection

Operating guide for the headless foundation. Design constraints are in
[architecture](../architecture.md); what is proven is in
[implementation status](../implementation-status.md).

## Run

Requires native Linux and Rust 1.99.1 (pinned with Rustfmt and Clippy). Bevy 0.20.0
uses headless features; no GPU, display, original game files or Proton are required.
Basic data comes from the free official dedicated server (Steam app 2089300),
fetched unattended and anonymously without steamcmd or the game client (F-828,
F-831), where `run` finds it when `network.data_pak` is unset (`network_listening`
reports `data_pak` and `data_pak_source`, `configured` or `installed`):

```bash
target/release/icarus-server install-official-data --data-dir /var/lib/icarus-native
# -> /var/lib/icarus-native/official/Icarus/Content/Data/data.pak and install.json
```

It prints and saves `official/install.json` (build, depot, manifest, size, SHA-1,
SHA-256); rerun after an official update. `icarus-fetch --app 2089300 --file PATH --out DIR`
fetches any official server file, and `--list` lists them.

```bash
cargo build --release --locked
cargo run --locked --bin icarus-server -- check-config --config config/server.example.toml
cargo run --release --locked --bin icarus-server -- run --config config/server.example.toml
```

Use SIGINT or SIGTERM to stop and save. For a bounded smoke run:

```bash
target/release/icarus-server run --data-dir work/smoke-world --max-ticks 10
```

For qualification, `--compressed` (requires `--max-ticks`) runs simulated time
without pacing: autosave and status intervals count simulated seconds and await
in-flight autosaves ([synthetic workload](synthetic-workload.md)).

The provisional development default is 30 Hz (at most four catch-up ticks per
update, not a discovered ICARUS rate). No sockets open unless `[network]` is
enabled. JSON lifecycle/status/save records go to stderr with `gameplay_ready: false`;
tick timings cover foundation systems only, not gameplay performance.

Configuration precedence is CLI → TOML → defaults; unknown fields and invalid
limits reject, and relative paths resolve from CWD. Config inputs must be an
opened regular file of at most 64 KiB (regular symlinks allowed; FIFOs and
oversized inputs fail before state opens, F-671). `check-config` prints effective
settings without creating `--data-dir`; see `--help` per subcommand.

## Network sessions (experimental)

An opt-in `[network]` table (disabled by default) runs the loopback peer lab
inside `run` (F-875):

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

Release profile and socket bind precede opening `--data-dir`; failure exits
nonzero, and mismatched Hello versions receive Upgrade. Updates poll at most 32
datagrams before fixed ticks; SIGINT/SIGTERM stops admission and closes sessions
before draining and saving. `network_listening` reports address and `version_check`;
`status` and `stopped` report `sessions` counts. Worlds seeded by `preserve-save`
replicate active prospect and game-mode state (`active_prospect_replicated`,
F-804); with `data_pak` (F-828), saved session flags, `LockedMissions` and
fresh-pawn vitals/inventories replicate from table rows (`unmapped_session_flags`,
`locked_missions`; F-830, F-835, F-836). No ICARUS client compatibility,
identity check, replication or gameplay is implied; only authored probes are tested:

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

For long holds, `--progress-interval-seconds 60` flushes `incomplete: true`
aggregate JSON lines to stderr while stdout keeps only the final summary.
Progress combines each session's latest receive window (including ongoing gaps),
not a synchronized snapshot, and cannot certify interrupted runs; write failures
stop probes after bounded work, and single-session setup/close errors report
error kinds only, like concurrent holds.

Keep the supervisor independent of the terminal and retain both child exit
receipts plus the service manager's final exit status. Verify actual namespaces,
affinity and cgroup limits: accepted user-service settings did not enforce
network isolation or CPU affinity on this host (M-1042). An explicit bubblewrap
network namespace and `CPUAffinity` passed a loopback probe and supervisor tests
(M-1043). Bind each run to a fresh scope, frozen inputs and a bounded owned unit;
an ongoing run establishes no duration result.

## Replication scheduling (experimental)

`replication::ReplicationPlugin` runs in `FixedUpdate` between gameplay and
tick finish, driving `experimental::actor_channel` consider/relevancy/priority
stages per `NetViewer` into `ReplicationFrame` with UE default net parameters
(not ICARUS values). Optional `SpatialMode::Grid` (off by default) matches the
naive relevant set; it sends nothing and models no bandwidth, dormancy,
ownership or destruction.

```bash
cargo run --release --locked --example replication_bench -- --viewers 1,4,8
# --source <envelope>: planned-actor positions; prints aggregates only
```

## Persistence and recovery

Default state location: ignored `work/runtime/`, single-writer advisory lock,
60 s bounded-worker autosave (authored domain records and clock only, not ICARUS
gameplay). Entities with a domain ID must have a position before capture (F-482).
Schema 1 stores records and clock (64 MiB); schema 3 adds a write-once,
digest-verified `preserved-<sha256>.bin` sidecar (schema 2 still loads, F-567).
Schemas 4–11 persist authored spatial worlds, C001 fixtures, relationships,
owner lifecycle, scalars/inventories and owner modifiers (F-919, F-922, F-923,
F-927, F-931; [authored native worlds](authored-worlds.md)) under a shared
64 MiB + 4 KiB generation cap (including inline sections and sidecar reference).
Unsupported schemas fail closed; preservation grants no import acceptance.

Generations are checksummed and synced before atomic manifest publication; two
generations are retained, recovering from previous on current-file corruption
while failing closed on corrupt/missing manifests, noncanonical digests,
unsupported schemas or other I/O errors (F-869, F-870, F-771). Back up before
manual recovery. SIGINT/SIGTERM freezes simulation, drains work and saves within
a deadline, exiting nonzero on failure (not a hard process-exit-time guarantee,
F-869; qualified for local Linux storage only, not network filesystems or
physical power loss, U-035).

## Supplied saves

All modes open a nonblocking regular file (regular symlinks allowed; F-671) and
write nothing unless stated:

- `inspect-save --source PATH`: [envelope checks](../specs/save-format/prospect-envelope.md);
  `--inner-tags` adds [root framing](../specs/save-format/tagged-properties.md),
  `--recorders` recorder framing (counts only).
- `--stage-check [--expect-prospect-name NAME]`: full `FProspectSaveState`
  staging, migration, `ProspectInfo` and base recorder fields (F-738).
- `--typed-check`: staging, then lossless recorder value typing and re-encoding;
  gaps counted by reason per class pseudonym (F-766).
- `--preserve-check`: temporary schema-3 save/reload check with sizes, timings
  and memory (F-567, F-873).
- `preserve-save --source … --data-dir …`: seeds a fresh tick-0 world holding
  only the preserved prospect; refuses an existing world.
- `import-save --source … --destination … --dry-run`: prints the dry-run import
  plan; without `--dry-run` writing is refused (F-898). `--catalog FILE`
  (regular TOML `<= 4` MiB, `format = 1`) maps `[references]` lists per role
  (`actor_class`, `actor_path`, `data_row`, `asset_path`, `world_tile`,
  `class_variable`, `unclassified`); a reference maps only in the roles that
  list it, and instance names need no list. `--data-pak PATH` also resolves
  `data_row` references case-insensitively against official server tables.
  After migration, `ProspectDTKey` must resolve in `D_ProspectList` or an
  authored `data_row` catalog, while `FactionMissionDTKey` and custom-setting
  rows use `data_row` resolution (`ProspectMapName` uses `unclassified` in
  format 1; empty optional refs do not block; F-898).

Catalog format 2 scopes world references (F-916):

```toml
format = 2
[references]
data_row = ["ExampleRow"]
[[world.classes]]
name = "ExampleClass"
variables = ["ExampleVariable"]
[[world.maps]]
name = "ExampleMap"
tiles = ["ExampleTile"]
[[world.maps.actors]]
path = "/Game/Authored/ExampleActor"
class = "ExampleClass"
```

Format 2 requires `[world]`, admits only `data_row`, `asset_path` and
`unclassified` under `[references]`, scopes actor paths/tiles to the declared
root map and variables to declared classes (exact case, `<= 1,024` UTF-16 units
per ref, `<= 4` MiB file; duplicate definitions in one scope and undeclared
placement classes fail before planning; `data.pak` never satisfies world roles;
F-916). `importable: true` reports reference-planning coverage only (counts
only; no geometry, adapters or gameplay parity); writing remains refused
(`world_imported: false`).

`DecodedImportCandidate::decode` borrows into `plan_decoded_import`,
`planned_actors_from_candidate` and preservation, retaining migrated roots,
recorder outcomes and unknown enums under `256` MiB charged bytes and
`1,048,576` typed elements (releasing completed typing scratch; exhaustion
rejects all, even dropped records, and does not bound RSS; F-920).
`inspect-save --candidate-check --source prospect.json` reports counts, retained
charges, `required_decode_limits` and `remaining_headroom` (which cannot
guarantee another recorder's preflight) without exposing names/values or
granting import authority. Value meanings and original-server round trips remain
unvalidated; [synthetic workload](synthetic-workload.md) tests native
scheduling/durability only.
