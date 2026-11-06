# Network readiness diagnostics

[Setup](native-server.md#network-sessions-experimental). F-1012: no client
execution, spawn, geometry or world-readiness proof.

## Probe output

`native_join_probe`: stderr progress JSON every 5 s
(`--progress-interval-seconds`, 1–60); stdout final uses pre-shutdown counts.

`controller_fields` (final: `controller_fields_before_shutdown`): ≤64 target/
index/bit-count/disposition/count rows per peer/aggregate, plus overflow/malformed
counts; no payloads/identities. Removal drops rows. Recognition is not application;
malformed tails apply nothing; overflow prevents absence claims.

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
exactly during staging. Transient reassembly: ≤16 assemblies × 16 KiB.
Packets/unrelated values are never persisted; exports/must-map/open/close/paused
bunches are unsupported.

Private JSONL: exact bit count/masked-tail hex; neutral observation/connection
ordinals; channel; completion elapsed ms; engine net version/archive byte order;
reliable/partial/reassembled flags and part count; block/field/payload bit
offsets/lengths relative to the complete bunch payload. No datagram offsets,
fragment times/offsets or parameter-presence framing. Put release/executable
hashes in the private case manifest. Never publish raw contents or log them.

Progress/final `private_rpc82_capture` exposes only counts: pending assemblies,
drops, malformed input, write errors, `incomplete`. This flag stays 1 while
running or after loss, unfinished assembly or I/O error; completion needs file
sync and final summary. Write errors fail the probe after shutdown; a partial
final JSONL row is incomplete.

## Readiness counts

Receipts need a positive ACK on an open/nonclosing channel and latch until
peer removal. Closed unsettled channels stay awaiting, even after channel reuse.

`initial_open_outcomes`: configured, committed (queued), settled, awaiting_ack,
failed, skipped, pending, deferred per controller/pawn/GameState/PlayerState/
managers/quests. `sequences_succeeded`: no failures; else `sequences_failed`.
Settlement also needs a reliable-empty channel; unrelated traffic can delay it.
`sequences_settled`: successful sequences with all opens settled.

`initialisation_blockers`: incomplete players' `requests_pending_queue` and,
after admission, `awaiting_loadout_response`/`awaiting_bestiary_response`.
Blockers may overlap; `players_*` phases partition.

`initialisation_transport`: `requests_{queued,settled,awaiting_ack}` for the
controller request batch and `completions_{queued,settled,awaiting_ack}` for
PlayerState character-ID publication. Queued means transport admission;
settlement needs every reliable sequence in the batch ACKed. Later same-channel
traffic does not delay it. Queued equals settled plus awaiting ACK,
independently of responses, application and finalisation.
