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
| M5: migration/hardening | Pending, native hardening underway | Real native filesystem faults pass; real saves preserve losslessly in native generations and run through autosaves (F-567, F-578); value meanings and migration fidelity remain unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

Offline work, in order; connected proof keeps its gates.

1. Save classes: with Map, Set and Int64 headers and a 1 MiB ceiling, every body of every class frames and round-trips in both copies, and every Map frames into items (F-556, F-557, F-559, F-562); the only opaque Structs are four fixed-width engine math types (F-565); next their component layout and value meanings (U-027, U-020).
2. Player state: candidate mission, mount, inventory and player classes frame except C016 over the 128 KiB ceiling (F-550); construction classes and field meanings remain; class and field names do not discriminate the producing build (F-498, F-547), so confirming it needs value-level or runtime evidence (F-262, F-277, U-040).
3. Control stores: owner, dispatcher, static virtual call site and base origin across the super calls are qualified (F-505, F-506, F-520, F-544); runtime dispatch needs a connected or original-runtime scope (U-043).
4. Control stores: constructors and notify tables correspond by position (F-505, F-546) and both builds' NotifyControlMessage functions correspond function-level (F-579); runtime dispatch still needs a connected or original-runtime scope (U-043).
5. Packet framing: formats, version 17 and the reliability layer are recorded with an experimental codec, sequencing model and per-peer state (F-519, F-521 to F-527, F-535 to F-539); the Bevy loopback peer lab runs them and answers Hello and Login without admission (F-558, F-564); the server's own Hello, Login and Welcome are read (F-563, F-580); the runtime version value and Welcome strings need connected evidence (U-037, U-044); admission stays gated.
6. Transport: the control vocabulary, Hello replies, challenge and unique-net-id format are mapped (F-529 to F-531), with the client's version construction and type table (F-541 to F-543); Login carries no credential, and no Steam packet-handler class is evidenced (F-548, F-549); next the runtime handler configuration, game-level identity checks and the client's runtime project version (F-253, U-037, U-038).
7. World data: both copies preserve losslessly in a write-once schema-3 sidecar (F-557, F-567); seeded worlds run autosaves without misses, holding the section in about 42 MB (F-578); next a streaming verifier to lower the 110 MB load peak, then multi-world runs (U-045).
8. Reliability: collect the running 72-hour synthetic soak at its end and record it against U-035; it stays synthetic, not ICARUS performance (F-485, U-035).
9. Replication: the client's actor spawn record, content blocks, rep-handle stream, NetGUIDs, spawn quantization and common item and name encodings are read (F-569 to F-575); an experimental codec re-encodes them bit-identically (F-577); the server's content-block and NetGUID writers mirror them (F-581); next native struct serializers and the server's send policy (U-064, U-044).
