# Network readiness diagnostics

[Setup](native-server.md#network-sessions-experimental). F-1012: no client
execution, spawn, geometry or world-readiness proof.

## Probe output

`native_join_probe`: progress every 5 s (`--progress-interval-seconds`, 1–60);
stderr progress JSON, stdout final pre-shutdown counts.

`controller_fields` (final: `controller_fields_before_shutdown`): ≤64 target/
index/bit-count/disposition/count rows per peer/aggregate, plus overflow/malformed
counts and `unknown_subobject_blocks`; no payloads/identities. Removal drops
rows. Recognition is not application; malformed tails apply nothing; overflow
prevents absence claims.

`sessions.profile_{user,character}_responses_recognized` counts parsed profile
calls dispatched after whole-bunch validation, including failure/wrong-phase
calls.
Wrong phase also increments `profile_responses_ignored`; malformed bunches apply
no calls. `profile_{user,character}_publications_deferred` counts publication
attempts retained behind drainable transport pressure; retries add no ingress.
Existing `profile_{user,character}_responses_received` counts successful
PlayerState publication admission, not RPC ingress. A zero count alone cannot
prove no response. Unknown-subobject blocks are unattributed skipped blocks,
not identified profile traffic (F-1040).

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

Private JSONL: bit count/masked-tail hex, neutral ordinals/channel, elapsed ms,
engine/order, reliable/partial/reassembly metadata and bunch-relative block/
field/payload spans; no datagram/fragment offsets or presence framing.
Keep release/executable hashes in the case; never publish raw contents.

Capture counts expose pending assemblies, drops, malformed input, write errors
and `incomplete`: 1 until final sync, or after loss, unfinished assembly/error.
Write errors fail after shutdown; a partial final JSONL row is incomplete.

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
