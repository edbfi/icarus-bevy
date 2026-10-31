# Native server: run, persistence and inspection

Headless foundation guide: [design constraints](../architecture.md) and
[proven behavior](../implementation-status.md).

## Run

Requires native Linux, pinned Rust 1.99.1 with Rustfmt/Clippy and headless
Bevy 0.20.1; no GPU, display or Proton. Protocol row identities are fetched
unattended/anonymously from the free official dedicated server (Steam app
2089300), without steamcmd or the game client (F-828, F-831). With
`network.data_pak` unset, `run` discovers installed data. Either source requires
`network.authored_gameplay` (F-993); `network_listening` reports
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
data_pak = "/path/to/IcarusServer/Icarus/Content/Data/data.pak"  # protocol row identities
authored_gameplay = "/path/to/authored-gameplay.json"  # required with configured/installed pak
```

Release profile/socket checks precede opening `--data-dir`; mismatched Hello
gets Upgrade. Updates poll ≤32 datagrams before fixed ticks. SIGINT/SIGTERM
closes sessions before draining/saving. `network_listening` reports
address/`version_check`; `status`/`stopped` report session counts. Preserved
prospects replicate active prospect/mode state (F-804). The pak supplies row
names/order; authored input supplies gameplay values (F-993).

`authored_gameplay` is a regular JSON file up to 8 MiB:
`{"schema_version":1,"tables":{"D_Example":[{"Name":"Example","Value":7}]}}`.
Each supplied table must match its catalog's complete row count, names and order.
The example is schematic, not a playable profile. Author values independently;
never export or copy original gameplay tables. Missing tables cannot fall back
to the pak. Omitted object fields retain the native parser's defaults; this
does not establish complete gameplay coverage or compatibility. Without either
input, the existing authored lab defaults remain available.

Each pawn open carries a distinct positive `IcarusUID` (handle 46, F-948).
Positive saved actor-recorder `IcarusActorGUID`s occupy the namespace first;
an unreadable one fails startup. Desired IDs count from 65,536 per world; the
search uses an authored xorshift64 PRNG (seed 1), not MSVC `rand`. Claim,
reservation and remap stores cap at 262,144 entries each. A failed claim skips
the pawn open and counts `open_errors`; session removal frees the UID. When a
joined client sends its character slot, `run` asks it for its user and
character profiles through its backend proxy and replicates them on its
`PlayerState`. Three failed responses close the session (the original kicks).
Temporary send pressure retains one unsent initialization step per
peer; ACK/NAK progress retries it without repeating queued steps. It then asks
for loadout and bestiary; once both have answered and the pawn is open, `run`
replicates `PlayerCharacterID`, and a saved
player character matching the login ID and the profile's slot gives the pawn its
saved UID once every joined controller has sent `ServerFriendsListUpdated`, as the
original finalises (F-973, F-975); `run` then sends the record's saved pawn state, with levels
and health clamped as the original does (F-964), and its saved inventories as
the original restores them, unless a default it cannot compute is missing
(F-979) with their weights (F-982), its saved modifiers (F-981) and its stats with the bounds they set (F-989).
Incomplete inventory/modifier/stat inputs withhold derived stats and saved
health, oxygen, food and water (F-989); independent fields still restore. A new player, whom no saved
controller matches, is added to the replicated History (F-967). That UID stays
claimed after the session ends. A client that
never answers never completes; the original's timeout only logs (F-959). A save
the strict staging refuses supplies no saved characters, controllers or
History: returning-player restore is off, and every joiner counts as new and is
added to the History the tracker starts empty with.

Explicit library APIs (F-995, F-998, F-1000) share reliable ACK gating,
coalescing, pressure retries and loss repair. `refresh_restored_modifier_times`
validates UID/raw/wire batches; `refresh_pawn_temperature` merges known fields
(`None` unchanged, zero explicit). `set_restored_modifier_time` and
`set_pawn_internal_temperature` run qualified setter order with caller-owned
effects before publication; temperature requires a known value and stat reads.

`configure_pawn_temperature_tick` requires all four temperatures and explicit
reference/cache flags, interval, elapsed time and fractional remainder.
`configure_restored_modifier_tick` requires an existing restored UID and
explicit owner role, interval enablement, elapsed time and interval.
`tick_pawn_temperature` / `tick_restored_modifier` accept elapsed time and
caller-owned effects, read current values and retain final values/timing.
Polling and retries never replay callbacks. No automatic climate, expiry,
destruction or persistence is inferred. Disconnect drops timing; reconnect
requires configuration again. Getters return copies.
Counts are exposed by `PeerLabStats` and `SessionCounts`.

Authored schema-4/7/11 hosts opt in with `[network].authored_replication` or
`run --authored-replication replication.json`. Disabled networking ignores it;
enabled networking needs a release session plan. Strict JSON (regular file
≤1 MiB, 1–1,024 bindings): `AuthoredReplication` in `src/native_replication.rs`.
Declare catalog/map, signed-axis/unit frame, archetype paths/checksums/`no_load`,
parameters and schedule. Only translation maps; quaternion must be `[0,0,0,1]`
and scale `[1,1,1]`. Proxies never enter persistence (F-938).

Format 1 accepts schema 4; format 2 pins `lifecycle_id` from a schema-7/11 snapshot.
Dormant slots activate when spawned, with any pending authored flag edits.
Format 2 defaults to `"owner_replacement": "retire_bridge"`: replacing a bound
owner retires the bridge. Opt into `"after_close_ack"` to replace owners while
unrelated actors continue. Each peer must ACK its old reliable close before the
latest incarnation opens with a fresh GUID; blocked replacements coalesce.
Old identities remain only while desired, open or awaiting ACK; GUID numbers
never recycle, and exhausted allocation refuses new opens.

Every tick validates the full source and proxy bindings, including with no peers.
Corruption retires the bridge and stops admission without editing native state.
`activated` counts live bindings exposed since installation. Modifier timers stay
native; this bridge sends no modifier containers. Generic lab reconfiguration,
original replacement semantics, saved-pose mappings (U-025), import, physics,
gameplay and stock-client behavior remain unqualified. Costs: schema 4 only
(F-939, F-941); lifecycle cost is unmeasured. Tests use authored probes:

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
