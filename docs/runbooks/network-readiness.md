# Network readiness diagnostics

[Setup](native-server.md#network-sessions-experimental). F-1045: no client
execution, spawn, geometry or world-readiness proof.

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

### Offline selected RPC82 replay

Checkout root; existing parents, new report:

```bash
python3 tools/cargo_guard.py build --locked --example capture_decode
python3 tools/cargo_guard.py --hold-target -- timeout 60s target/debug/examples/capture_decode selected-visibility work/<case>/selected-rpc82.jsonl work/<case>/replay.json --release-profile config/release.toml --archive normal-little-endian-package-map-custom --layout normal-single-custom-struct
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
`_sends_deferred`, `_errors` (abandoned). `unreliable_actor_bunches`: client
unreliable bunches since bind, all connections: per channel kind `bunches`,
`last_payload_bits`; `controller_fields` (16 max) `index`/`count`; no payloads.
`connected_player_records`: `registered`, `finalised`, `pending_send`
(connections with an unadmitted change); `creations_queued` =
`creations_settled` + `records_awaiting_ack`.
