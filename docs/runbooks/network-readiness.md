# Network readiness diagnostics

[Setup](native-server.md#network-sessions-experimental). F-1012: no client
execution, spawn, geometry or world-readiness proof.

## Probe output

`native_join_probe`: progress JSON on stderr every 5 s
(`--progress-interval-seconds`, 1–60); final stdout retains pre-shutdown counts.

`controller_fields` (final: `controller_fields_before_shutdown`): ≤64 target/
index/bit-count/disposition/count rows per peer/aggregate, plus overflow/malformed
counts; no payloads/identities. Removal drops rows. Recognition is not application;
malformed tails apply nothing; overflow prevents absence claims.

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
