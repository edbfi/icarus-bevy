# Native server: run, persistence and inspection

[Constraints](../architecture.md), [status](../implementation-status.md).

## Run

Toolchain: F-879. Fetch row identities anonymously (app 2089300; F-828, F-831).
Unset `network.data_pak` discovers installed data. Either source requires
`network.authored_gameplay` (F-993);
`network_listening` reports `data_pak`/`data_pak_source` (`configured`/`installed`):

```bash
target/release/icarus-server install-official-data --data-dir /var/lib/icarus-native
# -> /var/lib/icarus-native/official/Icarus/Content/Data/data.pak and install.json
```

Rerun after updates; `official/install.json` records provenance/hashes.
`icarus-fetch --app 2089300 --file PATH --out DIR` fetches files; `--list` lists them.

```bash
python3 tools/cargo_guard.py build --release --locked
python3 tools/cargo_guard.py --hold-target -- target/release/icarus-server check-config --config config/server.example.toml
target/release/icarus-server run --config config/server.example.toml
```

Smoke run:

```bash
python3 tools/cargo_guard.py --hold-target -- target/release/icarus-server run --data-dir work/smoke-world --max-ticks 10
```

`--compressed` requires `--max-ticks` and runs unpaced simulated time. Autosave
and status intervals use simulated seconds and await in-flight autosaves
([synthetic workload](synthetic-workload.md)).

Default: provisional 30 Hz, ≤4 catch-up ticks/update; sockets need `[network]`.
Lifecycle/status/save JSON: stderr, `gameplay_ready: false`; foundation timings.

Config: CLI → TOML → defaults; unknown fields/invalid limits reject; paths use
CWD. Require a regular file ≤64 KiB (symlinks allowed); reject FIFOs/oversized
files before state opens (F-671). `check-config` creates no `--data-dir`.

Builds: [guard](cargo-builds.md).

## Network sessions (experimental)

`native_join_probe`: public listener, one Steam CM session:
`cargo run --locked --example native_join_probe -- --name NativeResearch --seconds 300`.
Defaults: UDP 17777/27015, one player, authored empty world;
[connected-run case](../gates.md#before-any-original-server-or-client-run) required.
Listing/Auth/Join limits: F-1010, F-1009; entry stalls (U-079).
Ready's `authored_world`: `seed: 1`, `seed_initialised: true`, `flod_tile_count: 0`;
Flags: `--seed`, `--seed-initialised`, `--flod-tile-count`.
Startup: 1–32 authored actions, default none. Before sockets/TLS/Steam,
preflight checks world wire composition, count and release RPCs/field bound.
Progress: stderr JSON every 5 s (`--progress-interval-seconds`, 1–60); final
stdout: pre-shutdown counts. Readiness: queues/channels/phases (F-1012).
Password unvalidated; avoid real passwords.
`controller_fields` (final: `controller_fields_before_shutdown`): live peers,
≤64 rows per peer/aggregate: target kind, index, payload bits, disposition,
saturating count; overflow/malformed-bunch counts; no payloads/identities.
Removal drops rows. `Recognized` proves no application; malformed tails
apply no calls. Overflow prevents absence claims.
`readiness.initial_open_outcomes`: `controller`, `pawn`,
`game_state`, `player_state`, `managers`, `quests`: `configured`, `committed`
(queued), `failed` (terminal), `skipped` (unattempted after controller abort),
`pending`, `deferred` (drainable pressure). Retained until removal.
No failures: `sequences_succeeded`; else `sequences_failed`.
No ACK/spawn/geometry proof.
`readiness.initialisation_blockers`: `requests_pending_queue` counts deferred
initialisation batches; `awaiting_loadout_response` and
`awaiting_bestiary_response` count missing answers only after reliable queue
admission, for incomplete players. Gauges may overlap; `players_*` phases remain
exclusive.

`network.controller_startup` lists `flush-level-streaming`/`restart-pawn`;
CLI `--controller-startup` flags replace it in order. TOML `[]` disables startup.
Requires release/controller plans; restart needs a pawn.
Actions require successful opens/controller settlement; restart also waits for
its pawn's settlement/GUID. `network_listening` lists actions; readiness reports
`controller_startup_actions_pending` (unqueued), `_queued` (transport admission),
`_settled`, `_awaiting_ack` (queued minus settled), plus
`controller_startup_sequences_settled` and `_failed`.
Settlement requires a positive ACK leaving the whole controller channel open,
not closing, with no pending reliable bunches. Unrelated traffic can delay this
observation; later traffic retains it. A sequence settles when all configured
actions settle without failure. Removal drops counts; no execution/spawn/geometry
readiness is implied.

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
Match catalog row counts/names/order; author values independently, never copy
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
Unanswered clients never complete (F-959).
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
of authored records/clock. Domain-ID entities need positions (F-482).
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
