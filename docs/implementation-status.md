# Implementation status

The current checkpoint: what stands, milestone state and the next tasks. Claims
cite rows in the [findings ledgers](ledgers/findings/); open questions live in
the [unknowns ledger](ledgers/unknowns.md); preconditions live only in
[gates](gates.md).

Before restructure: 96ddb0b:docs/implementation-status.md

Last reconciled: 2026-10-15

## Summary

This is an independent native server foundation, not an ICARUS-compatible
server (F-487). One bounded stateless exchange has been observed against the
original (F-253). Protocol interoperability, authentication, gameplay,
world-data parity, official-save import and the release reliability and
performance targets are not demonstrated.

Offline research resumed under the user's execution instruction (F-404);
connected-client and original-runtime scopes remain unavailable. The user
reports the local client/save pair compatible, with authenticated and reference
access deferred (F-421).

What the foundation does is in the
[foundation digest](research/foundation/foundation.md). Research state is in the
[C001](research/save-format/c001-constructor.md),
[save-class](research/save-format/save-class-audits.md),
[control-store](research/network/control-stores.md) and
[actor-replication](research/network/actor-replication.md) digests.

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; isolated entry-map startup observed; supplied save integrity/outer format characterized; client identity checked (F-486); exact build pairing pending |
| M1: original bug reproduction | Pending | Offline startup did not reproduce reported crashes; no official bug is marked reproduced or fixed (F-217); controlled workloads still required |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; authenticated protocol, world-data and import feasibility gates remain open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.0 / Bevy 0.20.0 foundation tested; stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Real native filesystem faults pass; real saves preserve losslessly in native generations and run through autosaves (F-567, F-584); randomized inputs never panic the readers or the peer lab, and corrupted stores load a committed snapshot or fail closed (F-604 to F-607, F-611); value meanings and migration fidelity remain unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

Offline work, in order; connected proof keeps its gates.

1. Save classes: with Map, Set and Int64 headers and a 1 MiB ceiling, every body of every class frames and round-trips in both copies, and every Map frames into items (F-556, F-557, F-559, F-562); the only opaque Structs are four fixed-width engine math types of three or four ordered 4-byte components (F-565, F-602); next value meanings (U-020).
2. Player state: candidate mission, mount, inventory and player classes frame, C016 and C026 included once the ceiling rose to 1 MiB (F-550, F-557); construction candidates frame, building grids to every placed piece once struct arrays reach 1 MiB (F-633, F-634), and field meanings remain; no saved class or field name or recorder version discriminates the producing build, so confirming it needs value-level or runtime evidence (F-262, F-277, F-635, U-040).
3. Control stores: owner, dispatcher, static virtual call site and base origin across the super calls are qualified (F-505, F-506, F-520, F-544); runtime dispatch needs a connected or original-runtime scope (U-043).
4. Control stores: constructors and notify tables correspond by position (F-505, F-546) and both builds' NotifyControlMessage functions correspond function-level (F-579); runtime dispatch still needs a connected or original-runtime scope (U-043).
5. Packet framing: formats, version 17 and the reliability layer are recorded with an experimental codec, sequencing model and per-peer state (F-519, F-521 to F-527, F-535 to F-539); the Bevy loopback peer lab runs them and answers Hello and Login without admission (F-558, F-564); the server's own Hello, Login and Welcome are read (F-563, F-580); the runtime version value and Welcome strings need connected evidence (U-037, U-044); admission stays gated.
6. Transport: the control vocabulary, Hello replies, challenge and unique-net-id format are mapped (F-529 to F-531), with the client's version construction and type table (F-541 to F-543); Login carries no credential, and no Steam packet-handler class is evidenced (F-548, F-549); the game's login checks are the engine's capacity and net-id checks plus runtime-registered PreLogin delegates, now mirrored natively without Welcome (F-622, F-623); next the runtime handler configuration, those delegates and the client's runtime project version (F-253, U-037, U-038).
7. World data: both copies preserve losslessly in write-once schema-3 sidecars (F-557, F-567), and seeded worlds, alone or two at once, run autosaves without misses at 27.6 MB peak RSS (F-584); next value meanings for an interpreted import, which needs U-020 and the gates (U-045).
8. Reliability: compressed simulated time runs the 72-hour tick and save schedules in under an hour, synthetic and preserved-world alike (F-615, F-616); paced wall-clock effects and physical power loss remain (U-035), and none of it is ICARUS performance (F-485).
9. Replication: the client's actor spawn record, content blocks, rep-handle stream, NetGUIDs, spawn quantization and common item and name encodings are read (F-569 to F-575); experimental codecs re-encode spawn records, blocks and property streams bit-identically, the latter enforcing the client handle walk (F-577, F-586, F-587); the server's content-block and NetGUID writers mirror them (F-581); several native struct items, the movement struct, gameplay-tag packing, Name, Str and soft-object routing and hit results are read, and the codec re-encodes them with caller-supplied levels and widths (F-588 to F-600); the server composes actor bunches in client read order (F-610); Text items and their history bodies are read, client net archives carrying version 522, and the codec re-encodes every Text form (F-614, F-624, F-627, F-630); enum widths now derive from the enum maximum (F-632); layouts and tag tables need your call (U-064, U-065, U-066).
