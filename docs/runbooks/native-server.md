# Native server: run, persistence and inspection

Headless foundation guide: [design constraints](../architecture.md) and
[proven behavior](../implementation-status.md).

## Run

Requires native Linux, pinned Rust 1.99.1 with Rustfmt/Clippy and headless
Bevy 0.20.0; no GPU, display, original game files or Proton. Basic data is fetched
unattended/anonymously from the free official dedicated server (Steam app
2089300), without steamcmd or the game client (F-828, F-831). With
`network.data_pak` unset, `run` uses installed data; `network_listening` reports
`data_pak` and `data_pak_source` (`configured` or `installed`):

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

For a bounded smoke run:

```bash
target/release/icarus-server run --data-dir work/smoke-world --max-ticks 10
```

For qualification, `--compressed` (requires `--max-ticks`) runs simulated time
without pacing: autosave and status intervals count simulated seconds and await
in-flight autosaves ([synthetic workload](synthetic-workload.md)).

Development default: provisional 30 Hz, ≤4 catch-up ticks/update, not a
discovered ICARUS rate. Sockets require enabled `[network]`.
Lifecycle/status/save JSON goes to stderr with `gameplay_ready: false`;
tick timings cover foundation systems only, not gameplay performance.

Configuration: CLI → TOML → defaults; unknown fields/invalid limits reject;
relative paths use CWD. Config must open as a regular file ≤64 KiB (regular
symlinks allowed; FIFOs/oversized files fail before state opens, F-671).
`check-config` prints effective settings without creating `--data-dir`;
see each subcommand's `--help`.

## Network sessions (experimental)

`[network]` enables `run`'s loopback peer lab (default off, F-875):

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

Release profile/socket checks precede opening `--data-dir`; mismatched Hello
gets Upgrade. Updates poll ≤32 datagrams before fixed ticks. SIGINT/SIGTERM
closes sessions before draining/saving. `network_listening` reports
address/`version_check`; `status`/`stopped` report session counts. Preserved
prospects replicate active prospect/mode state (F-804); `data_pak` supplies
session flags, `LockedMissions`, fresh-pawn vitals/inventories (F-830, F-835, F-836).

Each pawn open carries a distinct positive `IcarusUID` (handle 46, F-948).
Positive saved actor-recorder `IcarusActorGUID`s occupy the namespace first;
an unreadable one fails startup. Desired IDs count from 65,536 per world; the
search uses an authored xorshift64 PRNG (seed 1), not MSVC `rand`. Claim,
reservation and remap stores cap at 262,144 entries each. A failed claim skips
the pawn open and counts `open_errors`; session removal frees the UID.

For authored schema-4 worlds, set `authored_replication = "replication.json"`
in `[network]` or pass `run --authored-replication replication.json`.
Ignored while networking is disabled. Enabled networking requires a release
profile with a session plan.
Strict JSON format 1: regular file ≤1 MiB, 1–1,024 bindings; full schema:
`AuthoredReplication` in `src/native_replication.rs`. Explicit declarations:
catalog/map identity, signed-axis/unit frame, archetype paths/checksums/`no_load`,
per-actor parameters and schedule. Poses require quaternion `[0,0,0,1]` and
scale `[1,1,1]`; only translation maps through the frame, not rotation/velocity.
All bindings/shared static identities validate before installation; proxies
are transient, never saved. Invalid bindings retire activation without native
state edits (F-938). Every fixed tick validates bindings, even without peers:
direct ECS corruption must retire activation and stop admission on the next
tick, independently of connection count. F-939 (debug) and F-941 (release)
measure this integrity cost.
Generic later lab reconfiguration is outside this contract.
Saved-pose mappings (U-025), import, physics, gameplay and stock-client behavior
remain unqualified; tests use authored probes only:

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

`--seconds` accepts up to 72 h 2 min (259,320 s): each session's clock starts
at its own Join, so 72 hours after full admission needs the margin.
For long holds, `--progress-interval-seconds 60` flushes aggregate
`incomplete: true` JSON lines to stderr; stdout holds only the final summary.
Progress combines latest per-session receive windows, including ongoing gaps;
it is not synchronized and cannot certify interrupted runs. Write failures
stop probes after bounded work; single/concurrent holds report only error kinds
for setup/close errors.

Use a terminal-independent supervisor; retain both child exit receipts and the
service manager's final exit status. Verify namespaces, affinity and cgroup
limits: accepted user-service settings failed to enforce network isolation/CPU
affinity here (M-1042). Explicit bubblewrap network namespace and `CPUAffinity`
passed a loopback probe/supervisor tests (M-1043). Each run needs a fresh scope,
frozen inputs and bounded owned unit; ongoing runs prove no duration.

## Replication scheduling (experimental)

`replication::ReplicationPlugin` runs between gameplay/tick finish in
`FixedUpdate`: `experimental::actor_channel` consider/relevancy/priority stages
per `NetViewer` produce `ReplicationFrame` with UE defaults, not ICARUS values.
Optional `SpatialMode::Grid` (default off) matches the naive relevant set.
Scheduling sends nothing; bandwidth/dormancy/ownership/destruction are unmodeled.

```bash
cargo run --release --locked --example replication_bench -- --viewers 1,4,8
# --source <envelope>: planned-actor positions; prints aggregates only
cargo run --release --locked --example authored_host_bench # F-938 vs no bridge
# --peers N (0..=8) joins authored loopback peers; each opens at most 250 actors
```

## Persistence and recovery

State defaults to ignored `work/runtime/`: single-writer advisory lock,
60 s bounded-worker autosave of authored domain records/clock only, not ICARUS
gameplay. Domain-ID entities need positions before capture (F-482).
Schema 1: records/clock, 64 MiB. Schema 3 adds a write-once, digest-verified
`preserved-<sha256>.bin` sidecar; schema 2 still loads (F-567).
Schemas 4–11: authored spatial worlds, C001 fixtures, relationships, owner
lifecycle, scalars/inventories and owner modifiers (F-919, F-922, F-923, F-927,
F-931; [authored native worlds](authored-worlds.md)), sharing a 64 MiB + 4 KiB
generation cap including inline sections and sidecar reference. Unsupported
schemas fail closed; preservation grants no import acceptance.

Generations are checksummed/synced before atomic manifest publication. Two generations remain;
current-file corruption recovers the previous one. Corrupt/missing manifests,
noncanonical digests, unsupported schemas and other I/O errors fail closed
(F-869, F-870, F-771). Back up before manual recovery. SIGINT/SIGTERM freezes
simulation, drains work and saves by a deadline; failure exits nonzero.
No hard process-exit-time guarantee (F-869); qualified only for local Linux
storage, not network filesystems/physical power loss (U-035).

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
- `import-save --source … --destination … --dry-run`: prints an import plan;
  writing without `--dry-run` is refused (F-898). `--catalog FILE`: regular
  TOML `<= 4` MiB, `format = 1`, with `[references]` lists for `actor_class`,
  `actor_path`, `data_row`, `asset_path`, `world_tile`, `class_variable`,
  `unclassified`. Each reference maps only in listed roles; instance names
  need no list. `--data-pak PATH` resolves `data_row` case-insensitively against
  official server tables. After migration, `ProspectDTKey` must resolve in
  `D_ProspectList` or authored `data_row` catalog; `FactionMissionDTKey`/custom-setting
  rows use `data_row`; format-1 `ProspectMapName` uses `unclassified`.
  Empty optional refs do not block (F-898).

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

Format 2 requires `[world]`; `[references]` allows only `data_row`, `asset_path`,
`unclassified`. Actor paths/tiles scope to the declared root map, variables to
declared classes: exact case, `<= 1,024` UTF-16 units/ref, `<= 4` MiB file.
Duplicate in-scope definitions/undeclared placement classes fail before planning;
`data.pak` cannot satisfy world roles (F-916). `importable: true` means reference
coverage counts only, no geometry/adapters/gameplay parity; writing stays
refused (`world_imported: false`).

`DecodedImportCandidate::decode` lends migrated roots, recorder outcomes and
unknown enums to `plan_decoded_import`, `planned_actors_from_candidate` and
preservation. Limits: `256` MiB charged bytes, `1,048,576` typed elements;
completed typing scratch releases. Exhaustion rejects all, even dropped records;
RSS is not bounded (F-920). `inspect-save --candidate-check --source prospect.json`
reports counts, retained charges, `required_decode_limits` and
`remaining_headroom` without names/values or import authority. Headroom cannot
guarantee another recorder's preflight. Value meanings/original-server round
trips remain unvalidated; [synthetic workload](synthetic-workload.md) tests only
native scheduling/durability.
