# Network readiness diagnostics

[Setup](native-server.md#network-sessions-experimental). F-1045: no client
execution, spawn, geometry or world-readiness proof.
Seeded lossy-join campaign (F-1169), about 10 minutes:
`ICARUS_LOSSY_JOIN_SEEDS=500 python3 tools/cargo_guard.py test --locked --lib lossy_join::extended -- --ignored --nocapture`;
a failing seed prints its `reproduce_one` command. A full sent ring answers
each client packet with its newest datagram again (`ack_ring_probes_sent`,
N-021); the ACK history clamps at eight words (N-020).

## Probe output

`native_join_probe`: progress every 5 s (`--progress-interval-seconds`, 1–60);
stderr progress JSON; stdout final `*_before_shutdown` counts plus
`sessions_after_shutdown` (shutdown failures/closes).

`controller_fields` (final: `controller_fields_before_shutdown`): ≤64 target/
index/bit-count/disposition/count rows per peer/aggregate, plus overflow/malformed
counts and `unknown_subobject_blocks`; no payloads/identities. Removal drops
rows. Recognition is not application; malformed tails apply nothing; overflow
prevents absence claims. Body failures: `body_parse_failures`; above cap with
bits present: `block_payload_limit_rejections`, largest in
`max_rejected_block_payload_bits`; past available bits:
`block_payload_length_rejections` (F-1045).

`sessions.profile_{user,character}_responses_recognized` counts parsed profile
calls dispatched after whole-bunch validation, including failure/wrong-phase
calls.
Wrong phase also increments `profile_responses_ignored`; malformed bunches apply
no calls. `profile_{user,character}_publications_deferred` counts publication
attempts retained behind drainable transport pressure; retries add no ingress.
Existing `profile_{user,character}_responses_received` counts successful
PlayerState publication admission, not RPC ingress. A zero count alone cannot
prove no response. Unknown-subobject blocks are unattributed skipped blocks,
not identified profile traffic (F-1045).

### Private selected RPC82 capture

`--private-rpc82-output work/<case>/selected-rpc82.jsonl` opts in; omission
installs no sink/summary fields. Before sockets/TLS/Steam, Linux `openat2`
requires an existing parent under checkout `work/`, refuses symlinks/traversal
and creates exclusively, mode 0600; unsupported kernels fail closed.
RPC82 remains `Ignored`, without a decoder or partial dispatch.

Persist only controller-channel actor field 82 after reliable ordering/duplicate
suppression and whole-bunch validation: malformed tails publish no prefix.
Length is not a filter (including 612 bits); retain the first payload within
ceilings: 16 observations, 8 KiB/field, 256 KiB encoded JSON/hex/newlines charged
exactly during staging. Transient reassembly: 128 KiB/assembly, 16 assemblies,
256 KiB aggregate (F-1042).
Packets/unrelated values are never persisted; exports/must-map/open/close/paused
bunches are unsupported.

RPC82 JSONL retains masked-tail hex, ordinals/channel/time, engine/order,
reassembly metadata and bunch/field/payload spans; no datagram/fragment offsets
or presence assertion. Keep executable/release hashes and files private.

Counts include assemblies/drops/malformed/write errors. `incomplete` stays 1
before final sync or after loss, unfinished assembly/error. Partial final rows
are incomplete; write failures fail after shutdown.

### Private controller subobject headers

`--private-controller-headers-output work/<case>/headers.jsonl` selects the
same private sink/bounds, excluding the RPC82 option. Actor-only blocks consume
no budget. JSONL retains GUID plus configured-component relation, header spans
(including packed payload length), flags and payload length; no payloads, names
or field indices. Client-to-server class/stable fields are null: absent on wire.
Raw packet sequence identifies the receive releasing the reliable queue, not
original arrival. First/final raw reliable sequences and release elapsed time
retain that distinction for reassembly; offsets are complete-bunch-relative.
Public `private_controller_headers_capture` contains counts only; retained
payload bits stay zero (F-1042). Keep files private and bind executable/release.

### Private pawn-channel bunches

`--private-pawn-bunches-output work/<case>/pawn-bunches.jsonl` opts in; it
combines with either selection above, under the same path rules and mode 0600.
`run` has no such flag or key. The tap reads each client bunch on the player's
own open pawn channel as the session releases it (reliable: in channel order,
once, part by part; unreliable: on arrival), before any lab handling, and
changes nothing sent or done. It keeps no decoder: the pawn's RPC field
indices, such as the frozen-movement report's (F-1100), are not bound.

JSONL `format` 2 (fields and rules: `src/experimental/selected_rpc_capture/pawn_bunches.rs`)
keeps receive times (lab ms, Unix ms), connection ordinal, channel, packet
sequence, reliable sequence (raw and in order), every header flag, exact
`payload_bits` and masked wire-order `payload_hex`: bit-exact for offline
decoding.

Per connection: the first 512 bunches while the first pool has room; every
reliable bunch; and per unreliable shape (flags, length, first 32 bits; up to
256 shapes) its first 4 bunches, then one per max(10 s, 2 s x tracked shapes).
After two connections fill the first pool, a reconnect's early unreliable
bunches fall back to the shape rules (`first_pool_full`; not a loss). Shapes past 256 are
counted (`untracked_shape_bunches`) and share one 10 s interval. Per file,
disjoint pools (rows / MiB): reliable 2,048 / 4, first 1,024 / 2, new shape
2,048 / 4, interval 3,072 / 6 (8,192 rows, 16 MiB); 8 KiB payload per row.
Frequent traffic thus cannot crowd out a reliable report or a new shape. At 83
bunches/s, interval rows last 8.5 h for one shape, 2.8 h for three alternating
and 1.7 h for 200 (small rows; larger ones exhaust bytes sooner); reliable and
new-shape rows remain kept after that (unit-tested).
Counts (`private_pawn_bunches_capture` on progress, `native_join_frozen_movement`
and final lines): `seen`, `kept_*`, `sampled_out` and `first_pool_full` (by
design), `lost_reliable`, `lost_new_shape`, `lost_interval`, `oversize`,
`incomplete`. Alarm on `lost_to_cap > 0` (their sum), live; sampling and
reconnects never raise it or `incomplete`. `connection_seen` and `shape_seen` show gaps.

### Offline selected RPC82 replay

Checkout root; existing parents, new report. Copy the complete frozen profile
of the capture's release (`tests/fixtures/releases/3.0.34.toml` or
`3.0.35.toml`) to `work/<case>/release.toml`; pin-only files are refused. The
tracked 3.0.36 profile is not accepted for this replay.

```bash
python3 tools/cargo_guard.py build --locked --example capture_decode
python3 tools/cargo_guard.py --hold-target -- timeout 60s target/debug/examples/capture_decode selected-visibility work/<case>/selected-rpc82.jsonl work/<case>/replay.json --release-profile work/<case>/release.toml --archive normal-little-endian-package-map-custom --layout normal-single-custom-struct
```

Caller asserts archive/layout; null-map:
`normal-little-endian-null-map-custom`. Input/report stay under `work/`;
openat2 refuses symlinks/traversal; report exclusive/0600.
No fallback/dispatch. Use a new report path per attempt; failures retain empty/partial reports.
Stdout/report: counts; stderr: fixed codes. Exit 0: nonempty, all conditionally
decoded; 2: empty/rejected; 1: failure (including help requests).
Completeness/provenance/compatibility remain unverified; bind sync/final summary/
hashes in the case.

`sessions.friends_list_queries_queued`: reliable field118 admissions, including
requeries when another player begins. It is not client execution.
`friends_lists_received` and `players_finalised` remain separate (F-1051).
`friends_list_queries_acknowledged`: a positive packet ACK settled that query's
own reliable bunch, once per query; delivery, not client execution.
`friends_list_rejections.*`: refused field126 payloads by fixed reason
(`truncated`, `array_count_over_limit`, `entry_limit_exceeded`,
`string_length_over_limit`, `string_missing_terminator`,
`string_invalid_encoding`, `trailing_bits`, `unclassified`); each also counts in
`actor_bunches_malformed`, drops its bunch and sends no requery.
`friends_lists_ignored_{unregistered,already_ready}`: parsed responses ignored,
also counted in `initialisation_calls_ignored`.
`connected_player_*` (sessions/stats, F-1077): `records_created` at Begin;
`record_creations_sent` per record and connection, `_acked` once its bunch is
ACKed (packet delivery, not client application); `record_updates_sent` 0 in
Mode A; `array_updates_sent` includes removals; `record_sends_deferred`
(pressure); `records_removed`; `record_errors`: undrainable refusals.

## Readiness counts

Receipts latch positive ACKs on open/nonclosing channels until removal.
Closed unsettled channels await even after reuse.

`initial_open_outcomes`: configured, committed (queued), settled, awaiting_ack,
failed, skipped, pending, deferred per controller/pawn/GameState/PlayerState/
managers/quests. `sequences_succeeded`: no failures; else `sequences_failed`.
Settlement also needs a reliable-empty channel; unrelated traffic may delay it.
`sequences_settled`: successful sequences with all opens settled.

`initialisation_blockers`: incomplete players' `requests_pending_queue` and,
after admission, `awaiting_loadout_response`/`awaiting_bestiary_response`.
Blockers may overlap; `players_*` phases partition.

`initialisation_transport`: `requests_{queued,settled,awaiting_ack}` for the
controller request batch and `completions_{queued,settled,awaiting_ack}` for
PlayerState character-ID publication. Queued means transport admission;
settlement needs every reliable sequence in the batch ACKed. Later same-channel
traffic does not delay it. Queued = settled + awaiting ACK, independent of responses/application/finalisation.

`friends_query` (live peers, latest admitted query each): `pending_admission`
held back from transport; `queued` = `settled` + `awaiting_ack`, where settled
is the ACK above and a newer admission replaces an unsettled receipt;
`awaiting_response`: settled, no newer query pending, no accepted field126
since.
`controller_possession` (F-1088): `pending_send`; `queued` = `settled` +
`awaiting_ack`. Counters `controller_possession_updates_{sent,acked}`,
`_sends_deferred`, `_errors` (abandoned).
`frozen_movement_release` (F-1099): `awaiting_delay`, `pending_send` (due or
NAKed; waits for unACKed reliable pawn bunches or pressure), `awaiting_ack`,
`settled`. Counters `frozen_movement_releases_{scheduled,sent,acked}`,
`frozen_movement_release_{resends,sends_deferred,errors}`,
`pawn_bunches_received_{reliable,unreliable}` (headers only). Probe
`frozen_movement` per session: `record_acked_ms`, `release_{due,sent,acked}_ms`,
`release_failed`, `pawn_inbound`, `pawn_moves`, `last_move_ack_ms`,
`spawn` ([spawn placement](#spawn-placement)).
`pawn_move_acks` (F-1127, F-1128): `sessions_moving`, `awaiting_ack`,
`deferred`, `positions_held`. Counters `pawn_moves_{received,malformed}`,
`pawn_move_acks_sent`, `pawn_move_ack_{sends_deferred,errors}`,
`pawn_move_positions_{accepted,rejected,skipped}`. `unreliable_actor_bunches` (probe
opt-in): client unreliable bunches since bind, all connections: per channel kind `bunches`,
`last_payload_bits`; `controller_fields` (16 max) `index`/`count`; no payloads.
`level_visibility` (probe-enabled, gated): `levels_visible`,
`levels_with_actors`, `opens_{pending,awaiting_ack,settled,refused_waiting}`,
`channels_held`, `channel_budget_remaining` (cap 128, 64 kept free). Counters
`level_visibility_reports_{added,duplicate,hidden,over_bound,malformed,refused_name,absent}`
(a malformed report is skipped, never its bunch) and
`level_actor_opens_{sent,acked,deferred,refused,requeued,withdrawn}`,
`level_actor_open_errors`, `level_hides_actors_kept`; `refused` counts once per
cause. ACK is packet delivery, not client application.
`connected_player_records`: `registered`, `finalised`, `pending_send`
(connections with an unadmitted change); `creations_queued` =
`creations_settled` + `records_awaiting_ack`.

## Spawn placement

F-1139, F-1140. `[network.spawn] points = [{ x = 0.0, y = 0.0, z = 0.0, yaw =
0.0 }]` (centimetres, Unreal axes, Z up; yaw in degrees, optional), or probe
`--spawn-location x,y,z` with optional `--spawn-yaw` (one point: the probe
admits one player). Joins take the points round-robin; without points the
pawn and controller open at the origin, as before (on terrain in the two
connected runs, F-1149). Needs a release profile;
at most 64 points, each component within ±1,048,576 cm, yaw within ±360°,
refused before the socket binds. The yaw is reported, not sent.

Positions come only from the operator's own knowledge of the map or a
player's own save: the server ships no map data and reads no packaged
terrain, collision or tables (F-828). Nothing checks the ground: a point below
the client's terrain falls forever (the client never resets its own pawn),
and one outside every landscape tile may keep the loading screen (U-079).

`network_listening.spawn_points`; counters `pawn_spawns_{saved,configured,default}`
and `pawn_spawn_saved_refused`; probe ready and summary `spawn`
(`location`, `yaw`, `yaw_sent`, `source`) and `spawns_before_shutdown`
per session. A configured placement starts that session's move store (F-1128).

## Fresh-player stats

F-1157, F-1168. Without tables a pawn opens with no stats and a stock client
cannot walk. `fresh_player_gameplay` writes a deterministic minimal schema-2
profile: required catalog row identities, values from flags (lab numbers,
not ICARUS balance). Unrelated row inserts/removals/reorders leave its bytes unchanged:

```bash
python3 tools/cargo_guard.py build --locked --example fresh_player_gameplay
python3 tools/cargo_guard.py --hold-target -- target/debug/examples/fresh_player_gameplay \
  --data-pak PATH/data.pak --output work/<case>/fresh-gameplay.json
```

Speeds in cm/s: `--walk-speed` 600, `--sprint-speed` 900, `--crouch-speed`
300, `--swim-speed` 300, `--swim-sprint-speed` 450. Vitals: `--max-health`,
`--max-stamina`, `--max-oxygen`, `--max-water`, `--max-food` (100 each);
`--weight-capacity` 1,000,000. Each is 1–10,000,000, the weight capacity at
most 2,147,483 (the restore's Overburdened check multiplies it by 1,000,
F-986). Only the five speeds replicate; the maxima fill the character state
but are never sent as stats 0 and 3 (F-1166). An existing `--output` is never
replaced; a written profile the loader refuses is removed. Stdout:
`table_rows`, `fresh_pawn_stats` and `movement_stat_ids`, which must read 58,
61, 65, 69 and 73 (F-1153); never a packaged value. `--skeleton TABLE`
(repeatable) writes a full schema-1 names-only template instead.

Probe `--data-pak PATH --authored-gameplay PATH`, both or neither, as
`[network] data_pak`/`authored_gameplay`. Before sockets, TLS or Steam the
preflight loads both, needs the release's pawn profile and composes the pawn
open with every restore table the bind reads, so a profile lacking one (such
as `D_ModifierStates` while the pak holds it and `D_ItemsStatic`) fails
naming the table, quoting nothing of the file. Ready and summary
`data_tables`: `loaded`, `fresh_pawn_stats`; no path or value. Counter
`pawn_open_stats_sent`.

## Join password

F-1161 to F-1163, F-1167. `[network] join_password` (unset or empty: none) or
probe `--join-password-file work/<case>/join-password`: a regular file under
checkout `work/` (no symlinks or traversal), mode 0600, at most 1 KiB of
UTF-8, one trailing newline trimmed, not empty. Either way at most 64 UTF-16
units, no control characters or `?`. Only a protected probe listing carries
`L_s`, which makes a stock browser prompt; without a password nothing
prompts. Login compares the URL `password` option ignoring ASCII case
(missing is empty) before the capacity checks and refuses a mismatch with
`Invalid Password`. The password is never logged, serialised or echoed:
`check-config` omits it, records and probe lines show only `join_password`
true or false, and configuration errors give the rule and position, not the
text. `run` binds loopback only and lists nothing. Untried with a stock
client (U-081).
