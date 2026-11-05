# Native server: run, persistence and inspection

[Constraints](../architecture.md), [status](../implementation-status.md).

## Run

Toolchain: F-879. Anonymous row identities: app 2089300 (F-828, F-831).
`network.data_pak` defaults to installed data. Both require
`network.authored_gameplay` (F-993);
`network_listening` reports `data_pak`/`data_pak_source` (`configured`/`installed`):

```bash
target/release/icarus-server install-official-data --data-dir /var/lib/icarus-native
# -> /var/lib/icarus-native/official/Icarus/Content/Data/data.pak and install.json
```

Rerun after updates; provenance/hashes: `official/install.json`.
Fetch: `icarus-fetch --app 2089300 --file PATH --out DIR`; list: `--list`.

```bash
python3 tools/cargo_guard.py build --release --locked
python3 tools/cargo_guard.py --hold-target -- target/release/icarus-server check-config --config config/server.example.toml
target/release/icarus-server run --config config/server.example.toml
```

Smoke: add `--data-dir work/smoke-world --max-ticks 10` to `run`; retain the
[build guard's](cargo-builds.md) `--hold-target` lease.

`--compressed` needs `--max-ticks`: unpaced simulated time, including autosave/
status intervals; awaits autosaves ([workload](synthetic-workload.md)).

Provisional 30 Hz, ≤4 catch-up ticks/update; opt-in `[network]`.
Stderr JSON reports foundation timings; `gameplay_ready: false`.

CLI → TOML → defaults; paths use CWD. Unknowns/invalid limits reject. File: regular, ≤64 KiB, symlinks allowed; FIFOs/oversize reject before
state opens (F-671). `check-config` creates no state.


## Network sessions (experimental)

`native_join_probe`: public listener, one Steam CM session:
`cargo run --locked --example native_join_probe -- --name NativeResearch --seconds 300`.
Defaults: UDP 17777/27015, one player, empty world; requires a
[case](../gates.md#before-any-original-server-or-client-run).
Listing/Auth/Join: F-1010, F-1009; entry stalls (U-079).
`--seed` (1), `--seed-initialised` (true), `--flod-tile-count` (0) appear in
ready's `authored_world`. Startup: 1–32 actions, default none. World wire/count
and release RPC/bound preflight precedes sockets/TLS/Steam.
Progress: stderr JSON every 5 s (`--progress-interval-seconds`, 1–60);
final stdout retains pre-shutdown counts (F-1012).
Password unvalidated; use no real passwords.
`controller_fields` (final: `controller_fields_before_shutdown`): ≤64 rows per
peer/aggregate: target/index/bit count/disposition/count; overflow/malformed
counts; no payloads/identities. Removal drops rows. Recognized
need not mean applied; malformed tails apply nothing; overflow prevents absence claims.
Readiness (F-1012):

- `initial_open_outcomes`: controller/pawn/GameState/PlayerState/managers/quests
  counts: configured, committed (queued), settled, awaiting_ack, failed, skipped,
  pending, deferred. Managers aggregate opens. `sequences_succeeded`: no failures;
  else `sequences_failed`. Settlement requires a positive ACK
  with the channel open, nonclosing and reliable-empty; unrelated traffic can
  delay it. Closed unsettled channels cannot settle after reuse. Receipts latch
  until teardown. `sequences_settled`: successful sequences with all opens settled. No execution/world-readiness proof.
- `initialisation_blockers`: `requests_pending_queue`, plus missing
  `awaiting_loadout_response`/`awaiting_bestiary_response` after request queue
  admission, for incomplete players. Overlap allowed; `players_*` phases partition.

`[network.authored_world]` supplies authored bootstrap
inputs for `run`:

```toml
[network.authored_world]
seed = 1
seed_initialised = true
flod_tile_count = 0
```

Requires all fields and release/controller/GameState/FLOD plans. Counts 0..=65535 must fit both byte-order
codec budgets before binding. No tiles/terrain created (U-079).
`network_listening.authored_world` reports inputs when configured. Preserved
prospects conflict: reject after load, before simulation/polling; socket may
briefly bind, stored state unchanged. Disabled networking applies no bootstrap.

`network.controller_startup`: ordered `flush-level-streaming`/`restart-pawn`;
repeated CLI flags replace TOML; `[]` disables. Requires release/controller plans;
restart also needs a settled pawn/GUID. Successful opens precede actions.
`network_listening` lists actions. Readiness reports
`controller_startup_actions_{pending,queued,settled,awaiting_ack}` and
`controller_startup_sequences_{settled,failed}`. Pending means unqueued, queued
means transport admission, awaiting ACK means queued minus settled. Settlement
uses the open-channel ACK rule above; receipts persist until removal. All
actions must settle without failure. No execution/spawn/geometry proof.

`[network]` enables `run`'s loopback peer lab (default off, F-875):

```toml
[network]
enabled = true
bind = "127.0.0.1"          # IPv4 loopback only
port = 17777                # 0 selects an ephemeral port; CLI: --network-port
max_players = 8             # admission occupancy limit, 1..=64
session_timeout_seconds = 30  # idle/delivery timeout, 1..=600 s
release_profile = "config/release.toml"  # optional Hello version pin
data_pak = "/path/to/IcarusServer/Icarus/Content/Data/data.pak"  # protocol row identities
authored_gameplay = "/path/to/authored-gameplay.json"  # required with configured/installed pak
```

Profile/socket checks precede `--data-dir`; mismatched Hello gets Upgrade.
Poll ≤32 datagrams before fixed ticks. SIGINT/SIGTERM closes sessions, drains/saves.
`network_listening`: address/`version_check`; `status`/`stopped`: session counts.
Prospect/mode: F-804; data inputs: F-993. Monotonic delivery timeout counts
`closed_by_delivery_stall`/`peers_delivery_stalled` separately from idle expiry:
sent records without ACK/NAK progress, even with traffic (F-1013).

`authored_gameplay` is a regular JSON file up to 8 MiB:
`{"schema_version":1,"tables":{"D_Example":[{"Name":"Example","Value":7}]}}`.
Schematic, not playable. Match catalog row counts/names/order; author values independently, never copy
gameplay tables. Missing tables have no pak fallback; omitted fields use parser
defaults. Without either input, lab defaults apply. Compatibility unproven.

Pawn opens use distinct positive `IcarusUID`s (handle 46, F-948). Saved
`IcarusActorGUID`s reserve first; unreadable IDs fail startup. Claim/reservation/
remap stores cap at 262,144 each. Failed claims skip opens/count
`pawn_open_errors`; removal frees UID. Character slot triggers backend
user/character requests and `PlayerState` replication; three failures close
the session (original: kick).
ACK/NAK retries opens, profiles and one pending control reply per peer;
admission commits when queued. Begin waits for PlayerState; pawn retries keep
one UID claim (F-1011).
After loadout/bestiary answers and pawn open, `run` replicates `PlayerCharacterID`.
Matching login ID/profile slot restores saved UID once, then state after all joined
controllers send `ServerFriendsListUpdated` (F-973, F-975).
Restore limits: pawn clamps (F-964), inventory defaults (F-979), weights (F-982),
modifiers (F-981), stats (F-989). Incomplete inventory/modifier/stat inputs
withhold derived stats and saved health/oxygen/food/water; independent fields
restore. Unmatched players enter History (F-967); UID survives disconnect.
Unanswered clients never complete; original timeout logs only (F-959).
Rejected staging drops saved characters/controllers/History; joiners enter
empty History as new players.

Caller-owned `PeerLab` APIs need `enable_member_refresh`:
`complete_prospect_member_spawn` adds status-2, unsettled/not-playing members
(F-1014); `edit_prospect_member` retains ID spelling; `remove_prospect_member`
removes matches. IDs ASCII-fold; slots exact (original rule unknown).
Duplicate adds, missing removals and missing/ambiguous/unchanged edits return false.
Other members/time unchanged; atomic checks, ACK/loss retries.
Join refreshes playing flags. No automatic spawn/departure, save or advert update.

Explicit APIs (F-995, F-998, F-1000): ACK gating, coalescing, pressure/loss retries.
`refresh_restored_modifier_times` validates UID/raw/wire
batches; `refresh_pawn_temperature` merges fields (`None` unchanged, zero
explicit). `set_restored_modifier_time`/`set_pawn_internal_temperature` preserve
setter order and caller-owned effects before publication; temperature needs a
known value and stat reads.

`configure_pawn_temperature_tick`: all four temperatures, reference/cache flags,
interval, elapsed time and fractional remainder. `configure_restored_modifier_tick`:
existing restored UID, owner role, interval enablement, elapsed time/interval.
`tick_pawn_temperature`/`tick_restored_modifier` take elapsed time/caller-owned
effects and retain final values/timing from current state.
Polling/retries never replay callbacks or infer climate/expiry/destruction/persistence.
Disconnect drops timing; reconnect needs configuration. Counts:
`PeerLabStats`/`SessionCounts`.

Schema-4/7/11 hosts: `[network].authored_replication` or
`run --authored-replication replication.json`; ignored without networking,
otherwise requires a release session plan. `src/native_replication.rs`:
strict regular JSON ≤1 MiB, 1–1,024 bindings,
catalog/map, signed-axis/unit frame, archetype paths/checksums/`no_load`,
parameters/schedule. `frame.orientation` selects
`right_handed_zyx_degrees_v1`; omitted/`identity_only` requires `[0,0,0,1]`.
Scale stays `[1,1,1]`, velocity zero. [Rotation contract](../research/foundation/foundation.md#current-answer).
Proxies never persist (F-938).

Format 1: schema 4; format 2: pinned schema-7/11 `lifecycle_id`.
Dormant slots activate on spawn with pending flags. Format 2's default
`"owner_replacement": "retire_bridge"` retires on owner replacement.
`"after_close_ack"` keeps unrelated actors: each peer ACKs old reliable close
before latest incarnation opens with fresh GUID; blocked replacements coalesce.
Identities remain while desired/open/awaiting ACK. GUIDs never recycle;
exhaustion refuses opens.

Every tick validates all source/proxy bindings, even without peers. Corruption
retires the bridge/stops admission without native edits. `activated`: live bindings
exposed since installation. Timers stay native; modifier containers are not sent.
Unqualified: reconfiguration, original replacement, saved-pose mapping (U-025),
import, physics, gameplay, clients. Costs: schema 4 only (F-939, F-941);
lifecycle unmeasured.
Probes:

```bash
python3 tools/cargo_guard.py test --locked --test network
target/release/icarus-server run --config server.toml --data-dir work/net-smoke --max-ticks 600
cargo run --locked --example session_churn -- --server 127.0.0.1:17777 \
  --cycles 200 --concurrency 1 --release-profile config/release.toml
# Hold sessions and check GameState (F-833);
# --sessions N (default 1) holds N at once and prints the aggregate summary.
cargo run --locked --release --example session_hold -- --server 127.0.0.1:17777 \
  --release-profile config/release.toml --seconds 3600 --sessions 8
```

`--seconds` ≤259,320: per-Join clocks allow 72 h after admission.
`--progress-interval-seconds 60`: aggregate `incomplete: true` JSON on stderr,
final summary on stdout. Receive gaps cannot certify interrupted runs.
Write failures stop probes; setup/close errors report kinds only.

Use a terminal-independent supervisor; retain child/service-manager exits.
Verify namespaces, affinity and cgroups (failures: M-1042; qualified setup:
M-1043). Require fresh scope, frozen inputs and a bounded owned unit;
ongoing runs prove no duration.

## Replication scheduling (experimental)

`replication::ReplicationPlugin`: `FixedUpdate` between gameplay/tick finish;
`experimental::actor_channel` consider/relevancy/priority per `NetViewer`
produces `ReplicationFrame` with UE defaults, not ICARUS values.
`SpatialMode::Grid` (default off) matches naive relevancy. No sending or
bandwidth/dormancy/ownership/destruction model.

```bash
cargo run --release --locked --example replication_bench -- --viewers 1,4,8
# --source <envelope>: planned-actor positions; prints aggregates only
cargo run --release --locked --example authored_host_bench # F-938 vs no bridge
# --peers N (0..=8) joins authored loopback peers; each opens at most 250 actors
```

## Persistence and recovery

State: ignored `work/runtime/`, single-writer lock, 60 s bounded-worker autosave
of authored records/clock, not ICARUS gameplay. Domain-ID entities need positions (F-482).
Schema 1: records/clock, 64 MiB. Schema 3 adds a write-once, digest-verified
`preserved-<sha256>.bin` sidecar; schema 2 still loads (F-567).
Schemas 4–11: [authored worlds](authored-worlds.md) (F-919, F-922, F-923, F-927,
F-931); 64 MiB + 4 KiB per generation including inline sections/sidecar reference.
Unsupported schemas fail closed; preservation grants no import acceptance.

Checksum/sync generations before atomic manifest publication; retain two.
Current-file corruption recovers the previous one. Invalid/missing manifests,
noncanonical digests, unsupported schemas and I/O errors fail closed
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

Format 2 scopes world references (F-916):

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

Format 2 needs `[world]`; `[references]` permits only `data_row`, `asset_path`,
`unclassified`. Actor paths/tiles scope to root map, variables to classes:
exact case, ≤1,024 UTF-16 units/ref, ≤4 MiB file. Duplicate definitions or
undeclared placement classes fail before planning. `data.pak` supplies no world
roles (F-916). `importable: true`: reference coverage only, no geometry/adapters/
gameplay parity; writes refused (`world_imported: false`).

`DecodedImportCandidate::decode`: planning/preservation; limits 256 MiB charged
bytes, 1,048,576 typed elements; typing scratch releases. Exhaustion rejects
all, including dropped records; RSS unbounded (F-920).
`inspect-save --candidate-check --source prospect.json`: counts, retained charges,
`required_decode_limits`/`remaining_headroom`, no names/values/import authority.
Headroom cannot guarantee another recorder's preflight. Meanings/original round
trips unvalidated; [workload](synthetic-workload.md).
