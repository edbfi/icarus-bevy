# Native server: run, persistence and inspection

Headless foundation: [constraints](../architecture.md),
[status](../implementation-status.md).

## Run

Headless toolchain: F-879. Protocol row identities fetch anonymously from
the free official server (Steam app 2089300), without steamcmd or game client
(F-828, F-831). With
`network.data_pak` unset, `run` discovers installed data. Either source requires
`network.authored_gameplay` (F-993); `network_listening` reports
`data_pak` and `data_pak_source` (`configured` or `installed`):

```bash
target/release/icarus-server install-official-data --data-dir /var/lib/icarus-native
# -> /var/lib/icarus-native/official/Icarus/Content/Data/data.pak and install.json
```

`official/install.json` records build, depot, manifest, size, SHA-1/SHA-256;
rerun after updates. `icarus-fetch --app 2089300 --file PATH --out DIR`
fetches official server files; `--list` lists them.

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

Default: provisional 30 Hz, ≤4 catch-up ticks/update, unqualified.
Sockets require enabled `[network]`.
Lifecycle/status/save JSON goes to stderr with `gameplay_ready: false`;
tick timings cover foundation systems only, not gameplay performance.

Config: CLI → TOML → defaults; unknown fields/invalid limits reject;
relative paths use CWD. Config must open as a regular file ≤64 KiB (regular
symlinks allowed; FIFOs/oversized files fail before state opens, F-671).
`check-config` prints settings without creating `--data-dir`.

Builds: [guard](cargo-builds.md).

## Network sessions (experimental)

`native_join_probe` uses a public listener and one native Steam CM session:
`cargo run --locked --example native_join_probe -- --name NativeResearch --seconds 300`.
Defaults: UDP 17777/27015, one player, authored empty world;
[connected-run case](../gates.md#before-any-original-server-or-client-run) required.
Listing: F-1010. Auth/Join: one account, two runs; entry stalls (F-1009; U-079).
Ready reports the configured `authored_world`: `seed: 1`,
`seed_initialised: true`, `flod_tile_count: 0`; stock-client entry is unqualified.
Progress JSON flushes to stderr every 5 s (`--progress-interval-seconds`, 1–60);
final stdout retains pre-shutdown counts. Readiness covers queues, channels and
player phases (F-1012). Password is unvalidated; use no real password.
`readiness.initial_open_outcomes` counts live peers by `controller`, `pawn`,
`game_state`, `player_state`, `managers`, `quests`: `configured`, `committed`
(queued), `failed` (terminal), `skipped` (unattempted after controller abort),
`pending`, `deferred` (pending under drainable pressure). Completed outcomes
remain until peer removal; `sequences_succeeded` requires no failure,
otherwise `sequences_failed` increments. Neither proves ACK, spawn or geometry readiness.

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
Updates poll ≤32 datagrams before fixed ticks. SIGINT/SIGTERM closes sessions before
draining/saving. `network_listening`: address/`version_check`; `status`/`stopped`:
session counts. Prospect/mode: F-804. Row identities/authored values: F-993.
Monotonic timeout removes peers with sent records but no ACK/NAK progress despite
inbound traffic: `closed_by_delivery_stall`/`peers_delivery_stalled` count these
apart from idle expiry. This is native policy, not original timing (F-1013).

`authored_gameplay` is a regular JSON file up to 8 MiB:
`{"schema_version":1,"tables":{"D_Example":[{"Name":"Example","Value":7}]}}`.
Schematic, not playable. Match catalog row counts/names/order; author values
independently, never copy gameplay tables. Missing tables have no pak fallback;
omitted fields use parser defaults. Coverage/compatibility unproven. Without
either input, lab defaults apply.

Pawn opens carry distinct positive `IcarusUID`s (handle 46, F-948).
Saved actor-recorder `IcarusActorGUID`s reserve positive IDs first; unreadable
IDs fail startup. Claim/reservation/remap stores cap at 262,144 entries each.
Failed claims skip the pawn open and count `pawn_open_errors`; peer removal
frees the UID. A joined client's character slot triggers backend-proxy requests
for user/character profiles, replicated on `PlayerState`. Three failed responses
close the session (the original kicks).
ACK/NAK retries opens, profiles and one pending control reply per peer;
admission commits when queued. Begin waits for PlayerState; pawn retries keep
one UID claim (F-1011).
After loadout/bestiary responses and pawn open, `run` replicates
`PlayerCharacterID`. Matching login ID/profile slot restores the saved UID once
all joined controllers send `ServerFriendsListUpdated` (F-973, F-975), then
saved pawn state with level/health clamps (F-964), inventories unless required
defaults are unavailable (F-979), weights (F-982), modifiers (F-981) and bounded
stats (F-989).
Incomplete inventory/modifier/stat inputs withhold derived stats and saved
health, oxygen, food and water (F-989); independent fields still restore. A new player, whom no saved
controller matches, is added to the replicated History (F-967). That UID stays
claimed after disconnect. Unanswered clients never complete; the original's
timeout only logs (F-959). Rejected staging disables returning-player restore:
no saved characters/controllers/History; all joiners enter an initially empty
History as new players.

Caller-owned `PeerLab` APIs need `enable_member_refresh`:
`complete_prospect_member_spawn` adds status-2, unsettled/not-playing members
(F-1014); `edit_prospect_member` retains ID spelling; `remove_prospect_member`
removes matches. IDs ASCII-fold; slots exact (original rule unknown).
Duplicate adds, missing removals and missing/ambiguous/unchanged edits return false.
Other members/time unchanged; atomic checks, ACK/loss retries.
Join refreshes playing flags. No automatic spawn/departure, save or advert update.

Explicit APIs (F-995, F-998, F-1000) share ACK gating, coalescing, pressure
retries and loss repair. `refresh_restored_modifier_times` validates UID/raw/wire
batches; `refresh_pawn_temperature` merges fields (`None` unchanged, zero
explicit). `set_restored_modifier_time`/`set_pawn_internal_temperature` preserve
setter order and caller-owned effects before publication; temperature needs a
known value and stat reads.

`configure_pawn_temperature_tick`: all four temperatures, reference/cache flags,
interval, elapsed time and fractional remainder. `configure_restored_modifier_tick`:
existing restored UID, owner role, interval enablement, elapsed time/interval.
`tick_pawn_temperature`/`tick_restored_modifier` take elapsed time/caller-owned
effects and retain final values/timing from current state.
Polling/retries never replay callbacks or infer climate, expiry, destruction or
persistence. Disconnect drops timing; reconnect needs configuration. Getters
return copies; counts use `PeerLabStats`/`SessionCounts`.

Schema-4/7/11 hosts: `[network].authored_replication` or
`run --authored-replication replication.json`; ignored without networking,
otherwise requires a release session plan. `src/native_replication.rs`:
strict regular JSON ≤1 MiB, 1–1,024 bindings,
catalog/map, signed-axis/unit frame, archetype paths/checksums/`no_load`,
parameters/schedule. `frame.orientation` selects
`right_handed_zyx_degrees_v1`; omitted/`identity_only` requires `[0,0,0,1]`.
Scale stays `[1,1,1]`, velocity zero. [Rotation contract](../research/foundation/foundation.md#current-answer).
Proxies never persist (F-938).

Format 1 accepts schema 4; format 2 pins schema-7/11 `lifecycle_id`.
Dormant slots activate on spawn with pending flags. Format 2 defaults to
`"owner_replacement": "retire_bridge"`: owner replacement retires the bridge.
`"after_close_ack"` preserves unrelated
actors: each peer ACKs its old reliable close before the latest incarnation
opens with a fresh GUID; blocked replacements coalesce. Identities remain while
desired/open/awaiting ACK. GUIDs never recycle; exhaustion refuses opens.

Every tick validates all source/proxy bindings, even without peers. Corruption
retires the bridge/stops admission without native edits. `activated` counts
live bindings exposed since installation. Timers stay native; no modifier
containers are sent. Generic reconfiguration, original replacement,
saved-pose mapping (U-025), import, physics, gameplay and stock clients remain
unqualified. Costs cover schema 4 only (F-939, F-941); lifecycle is unmeasured.
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

`--seconds` accepts up to 72 h 2 min (259,320 s): each session's clock starts
at its own Join, so 72 hours after full admission needs the margin.
Long holds: `--progress-interval-seconds 60` flushes aggregate `incomplete: true`
JSON to stderr; stdout holds the final summary. Progress merges unsynchronized
receive windows with ongoing gaps; it cannot certify interrupted runs. Write
failures stop probes after bounded work. Setup/close errors report kinds only.

Use a terminal-independent supervisor; retain child and service-manager exit
receipts. Verify namespaces, affinity and cgroup limits: accepted user-service
settings failed isolation/affinity (M-1042); explicit bubblewrap namespace and
`CPUAffinity` passed a loopback probe/supervisor tests (M-1043). Each run needs fresh scope, frozen
inputs and a bounded owned unit; ongoing runs prove no duration.

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
