# Native server: run, persistence and inspection

[Constraints](../architecture.md), [status](../implementation-status.md).

## Run

Toolchain F-879; anonymous identities: app 2089300 (F-828, F-831).
`network.data_pak`: configured or installed default; both need
`network.authored_gameplay` (F-993). `network_listening`: `data_pak`,
`data_pak_source` (`configured`/`installed`). Install:

```bash
target/release/icarus-server install-official-data --data-dir /var/lib/icarus-native
# -> /var/lib/icarus-native/official/Icarus/Content/Data/data.pak and install.json
```

After updates rerun; provenance/hashes: `official/install.json`.
Fetch: `icarus-fetch --app 2089300 --file PATH --out DIR`; list: `--list`.

```bash
python3 tools/cargo_guard.py build --release --locked
python3 tools/cargo_guard.py --hold-target -- target/release/icarus-server check-config --config config/server.example.toml
target/release/icarus-server run --config config/server.example.toml
```

Smoke: `run --data-dir work/smoke-world --max-ticks 10`; retain
[guard](cargo-builds.md) `--hold-target` lease.

`--compressed` requires `--max-ticks`: unpaced simulation/autosave/status time;
waits for autosaves ([workload](synthetic-workload.md)).

Provisional 30 Hz, ≤4 catch-up ticks/update; opt-in `[network]`.
Stderr JSON reports foundation timings; `gameplay_ready: false`.

CLI → TOML → defaults; paths use CWD. Unknowns/invalid limits reject.
Config: regular, ≤64 KiB, symlinks allowed; FIFO/oversize rejects before state
opens (F-671). `check-config` creates no state.


## Network sessions (experimental)

Public `native_join_probe`, one Steam CM session:
`cargo run --locked --example native_join_probe -- --name NativeResearch --seconds 300`.
Defaults: UDP 17777/27015, one player, empty world;
[case required](../gates.md#before-any-original-server-or-client-run).
Listing/Auth/Join F-1010/F-1009; entry stalls (U-079).
Ready `authored_world`: `--seed` (1), `--seed-initialised` (true),
`--flod-tile-count` (0). Startup: 1–32 actions, default none.
World wire/count/release RPC/bounds checked before sockets/TLS/Steam.
Password unvalidated: no real passwords.
Diagnostics: [network readiness](network-readiness.md) (F-1012).

Optional independent `run` bootstrap: `[network.authored_world]` with
`seed = 1`, `seed_initialised = true`, `flod_tile_count = 0`; omission keeps defaults.

All fields/release/controller/GameState/FLOD plans required; counts 0..=65535
must fit both byte-order budgets before binding. No terrain (U-079).
`network_listening.authored_world`: configured inputs. Preserved prospects:
reject post-load/pre-simulation/polling; socket may bind, stored state unchanged.
Networking off: no bootstrap.

`network.controller_startup`: ordered `flush-level-streaming`/`restart-pawn`;
CLI flags replace TOML; `[]` disables. Requires release/controller plans, plus
settled pawn/GUID for restart. Successful opens precede actions.
`network_listening`: actions. Readiness:
`controller_startup_actions_{pending,queued,settled,awaiting_ack}`,
`controller_startup_sequences_{settled,failed}`. Pending: unqueued; queued: admitted; awaiting ACK: queued minus settled.
All actions must settle without failure under the
[reliable-empty rule](network-readiness.md#readiness-counts).

`[network]` enables `run`'s loopback peer lab (default off, F-875):

Configure `[network]`:

| Key | Default/example; constraint |
| --- | --- |
| `enabled` | `true` to enable |
| `bind` | `"127.0.0.1"`; IPv4 loopback only |
| `port` | `17777`; `0`: ephemeral; CLI `--network-port` |
| `max_players` | `8`; admission occupancy, 1..=64 |
| `session_timeout_seconds` | `30`; idle/delivery, 1..=600 s |
| `release_profile` | `"config/release.toml"`; optional Hello pin |
| `data_pak` | `"/path/to/data.pak"`; protocol row identities |
| `authored_gameplay` | `"/path/to/gameplay.json"`; required with either pak |


Before `--data-dir`: profile/socket checks; mismatched Hello → Upgrade.
Poll ≤32 datagrams before fixed ticks. SIGINT/SIGTERM closes/drains/saves.
`network_listening`: address/`version_check`; `status`/`stopped`: session counts.
Prospect/mode F-804; data F-993. Monotonic delivery timeout: sent records lack
ACK/NAK progress despite traffic; `closed_by_delivery_stall`/`peers_delivery_stalled`
separate from idle expiry (F-1013).

`authored_gameplay`: regular JSON ≤8 MiB, e.g.
`{"schema_version":1,"tables":{"D_Example":[{"Name":"Example","Value":7}]}}`.
Schematic, unplayable/unqualified. Match catalog row counts/names/order; author
values, never copy gameplay tables. Missing tables: no pak fallback; omitted
fields: parser defaults; neither input: lab defaults.

Pawn UID (F-948): handle 46, ≤262,144 claims/reservations/remaps each.
Unreadable saved ID: startup failure; failed claim: skip open, count
`pawn_open_errors`; removal: free UID. Three character-slot profile exchange or
`PlayerState` replication failures close the session (original: kick).
ACK/NAK retries opens/profiles/one pending control reply per peer; queueing
commits admission. Begin waits for PlayerState; pawn retries retain one UID (F-1011).
Initialization/restore F-973/F-975; limits F-964/F-979/F-982/F-981/F-989. Incomplete inputs withhold derived stats/saved vitals; independent fields restore. Unmatched players enter History (F-967); unanswered clients never complete
(F-959). Rejected staging discards characters/controllers/History; joiners
enter empty History as new players.

`PeerLab` member APIs require caller `enable_member_refresh`:
`complete_prospect_member_spawn`: add status-2, unsettled/not-playing members
(F-1014); `edit_prospect_member`: retain ID spelling; `remove_prospect_member`: remove
matches. ASCII-fold IDs, exact slots (original rule unknown).
False: duplicate add, missing removal, missing/ambiguous/unchanged edit.
Atomic checks/ACK/loss retries preserve other members/time. Join refreshes
playing flags; no automatic spawn/departure/save/advert update.

Publication/setters: F-995, F-998, F-1000.
`refresh_restored_modifier_times` validates UID/raw/wire batches;
`refresh_pawn_temperature` merges fields (`None` unchanged, zero explicit).
`set_restored_modifier_time`/`set_pawn_internal_temperature` retain order/caller
effects; temperature needs known value/stat reads.

`configure_pawn_temperature_tick`: four temperatures, reference/cache flags,
interval/elapsed time/fractional remainder. `configure_restored_modifier_tick`:
restored UID, owner role, interval enabled, elapsed time/interval.
`tick_pawn_temperature`/`tick_restored_modifier`: elapsed time/caller effects,
final values/timing retained from current state.
Polling/retries: no callback replay or inferred climate/expiry/destruction/persistence.
Disconnect drops timing; reconfigure on reconnect. Counts: `PeerLabStats`/`SessionCounts`.

Schema-4/7/11: `[network].authored_replication` or
`run --authored-replication replication.json`; networking off: ignored; on:
release session plan required. `src/native_replication.rs`: strict regular JSON
≤1 MiB, 1–1,024 bindings,
catalog/map, signed-axis/unit frame, archetype paths/checksums/`no_load`,
parameters/schedule. `frame.orientation` selects
`right_handed_zyx_degrees_v1`; omitted/`identity_only` requires `[0,0,0,1]`.
Scale `[1,1,1]`, velocity zero; [rotation](../research/foundation/foundation.md#current-answer).
Proxies never persist (F-938).

Format 1: schema 4; format 2: pinned schema-7/11 `lifecycle_id`.
Spawn activates dormant slots with pending flags. Format 2 defaults to
`"owner_replacement": "retire_bridge"`: retire on owner replacement.
`"after_close_ack"`: unrelated actors stay; each peer ACKs old reliable close
before latest incarnation/fresh GUID opens. Blocked replacements coalesce.
Keep desired/open/awaiting-ACK identities; never recycle GUIDs; exhaustion refuses opens.

Each tick validates all source/proxy bindings, even peerless. Corruption
retires bridge/stops admission without native edits. `activated`: live bindings
exposed since installation. Timers stay native; no modifier containers sent.
Unqualified: reconfiguration/original replacement/saved pose (U-025)/import/
physics/gameplay/clients. Costs: schema 4 only (F-939, F-941); lifecycle unmeasured.
Probes:

```bash
python3 tools/cargo_guard.py test --locked --test network
target/release/icarus-server run --config server.toml --data-dir work/net-smoke --max-ticks 600
cargo run --locked --example session_churn -- --server 127.0.0.1:17777 \
  --cycles 200 --concurrency 1 --release-profile config/release.toml
# GameState hold (F-833); --sessions N (default 1): concurrent, aggregate summary.
cargo run --locked --release --example session_hold -- --server 127.0.0.1:17777 \
  --release-profile config/release.toml --seconds 3600 --sessions 8
```

`--seconds` ≤259,320: per-Join clocks, ≤72 h after admission.
`--progress-interval-seconds 60`: aggregate stderr JSON (`incomplete: true`),
stdout final summary. Receive gaps certify no interrupted duration.
Write failure stops probes; setup/close errors: kinds only.

Use a terminal-independent supervisor; retain child/service-manager exits.
Verify namespaces/affinity/cgroups (failures M-1042; qualified M-1043).
Fresh scope/frozen inputs/bounded owned unit required; ongoing runs prove no duration.

## Replication scheduling (experimental)

`replication::ReplicationPlugin`: `FixedUpdate`, between gameplay/tick finish.
`ReplicationFrame`: UE defaults, not ICARUS. `SpatialMode::Grid`: default off.
No sending/bandwidth/dormancy/ownership/destruction model.

```bash
cargo run --release --locked --example replication_bench -- --viewers 1,4,8
# --source <envelope>: planned-actor positions; prints aggregates only
cargo run --release --locked --example authored_host_bench # F-938 vs no bridge
# --peers N (0..=8) joins authored loopback peers; each opens at most 250 actors
```

## Persistence and recovery

State: ignored `work/runtime/`, single-writer lock, 60 s bounded-worker autosave
of authored records/clock; no ICARUS gameplay. Domain-ID entities need positions (F-482).
Schema 1: records/clock, 64 MiB; schema 3: plus write-once, digest-verified
`preserved-<sha256>.bin` sidecar; schema 2 loads (F-567).
Schemas 4–11: [authored worlds](authored-worlds.md) (F-919, F-922, F-923, F-927,
F-931); 64 MiB + 4 KiB per generation including inline sections/sidecar reference.
Preservation grants no import acceptance.

Checksum/sync before atomic manifest publication; retain two generations.
Corrupt current file recovers previous. Invalid/missing manifests,
noncanonical digests, unsupported schemas and I/O errors fail closed
(F-869, F-870, F-771). Back up before manual recovery. SIGINT/SIGTERM:
freeze simulation, drain/save by deadline; failure exits nonzero.
No hard exit-time guarantee (F-869); only local Linux storage qualified,
not network filesystems/physical power loss (U-035).

## Supplied saves

All modes: nonblocking regular file (symlinks allowed; F-671), read-only unless stated:

- `inspect-save --source PATH`: [envelope checks](../specs/save-format/prospect-envelope.md);
  `--inner-tags` adds [root framing](../specs/save-format/tagged-properties.md),
  `--recorders` recorder framing (counts only).
- `--stage-check [--expect-prospect-name NAME]`: stage/migrate full
  `FProspectSaveState`, `ProspectInfo`, base recorder fields (F-738).
- `--typed-check`: stage, losslessly type/re-encode recorder values;
  gap counts by reason/class pseudonym (F-766).
- `--preserve-check`: temporary schema-3 save/reload; sizes/timings/memory (F-567, F-873).
- `preserve-save --source … --data-dir …`: fresh tick-0 world, preserved
  prospect only; existing world rejects.
- `import-save --source … --destination … --dry-run`: print import plan;
  without `--dry-run` refuse writes (F-898). `--catalog FILE`: regular TOML
  ≤4 MiB, `format = 1`, `[references]` lists: `actor_class`,
  `actor_path`, `data_row`, `asset_path`, `world_tile`, `class_variable`,
  `unclassified`. References map only in listed roles; instance names need no list. `--data-pak PATH` resolves `data_row` case-insensitively against
  official server tables. After migration, resolve `ProspectDTKey` in
  `D_ProspectList` or authored `data_row` catalog; `FactionMissionDTKey`/custom-setting
  rows use `data_row`; format-1 `ProspectMapName` uses `unclassified`.
  Empty optional refs permit planning (F-898).

Format 2 world scope (F-916):

`format = 2` structure: `[references]`, `[[world.classes]]` (`name`,
`variables`), `[[world.maps]]` (`name`, `tiles`), `[[world.maps.actors]]`
(`path`, `class`).

Format 2 ([example](../../tests/import_plan.rs)) requires `[world]`; `[references]`: only `data_row`/`asset_path`/
`unclassified`. Scope actor paths/tiles to root map, variables to classes:
exact case, ≤1,024 UTF-16 units/ref, ≤4 MiB file. Duplicate definitions/undeclared
placement classes reject before planning. `data.pak`: no world roles (F-916).
`importable: true`: reference coverage, no geometry/adapters/gameplay parity;
writes refused (`world_imported: false`).

`DecodedImportCandidate::decode`: plan/preserve, ≤256 MiB charged bytes/
1,048,576 typed elements; release typing scratch. Exhaustion rejects all,
including dropped records; RSS unbounded (F-920).
`inspect-save --candidate-check --source prospect.json`: counts/retained charges,
`required_decode_limits`/`remaining_headroom`; no names/values/import authority.
Headroom cannot guarantee another recorder's preflight. Meanings/original round trips
unvalidated; [workload](synthetic-workload.md).

## Offline metadata tools

`tools/native_field_catalog.cjs`: call `joinNativeFieldCatalog` with three
reader reports, exported `PROFILE` and exact `options.addresses`; limits only
lower `CAPS`. Output stays `ready: false`.

`tools/pdb_selected_types.cjs`: use exported `PARAMETER_PROFILE` for bounded
parameter layouts with bases and embedded types. Declarations do not establish
runtime ancestry or consumers. Neither tool qualifies net order, checksums,
release activation or compatibility.
