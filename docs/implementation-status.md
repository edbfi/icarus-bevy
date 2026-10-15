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
offline original-server save-load runs are now authorized, connected-client
tests only per approved scenario ([gates](gates.md)). The user
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

1. Save classes: every body of every class frames and round-trips in both copies (F-557, F-565, F-602) and recorder fields have meanings (F-645 to F-661, F-665, F-666, F-676, F-677); Name identities and row meanings sit in packaged tables (F-660), whose use is your call (U-020).
2. Player state: candidates and construction grids frame (F-668, F-643); no saved identifier or version string discriminates the producing build (F-635, F-682); write times place the save in the 3.0.29 window, older than both installations (F-683, F-685), and the producing server's log would confirm it (U-040).
3. Control stores: owner, dispatcher, static call site, base origin, notify tables and both builds' NotifyControlMessage correspond statically (F-505, F-506, F-520, F-544, F-546, F-579); runtime dispatch needs a connected or original-runtime scope (U-043).
4. Packet framing and transport: formats, reliability, control vocabulary, unique net ids and the engine's login checks are recorded, with experimental codecs, a loopback peer lab and a native login mirror (F-519, F-521 to F-527, F-529 to F-531, F-535 to F-539, F-541 to F-543, F-548, F-549, F-558, F-563, F-564, F-580, F-622, F-623); next the runtime version value, handler configuration, PreLogin delegates and Welcome strings (U-037, U-038, U-044); the offline original accepts the framing and answers Hello with Challenge or Upgrade (F-688, F-689), so Login, Welcome and Join come next.
5. World data: both copies preserve losslessly and seeded worlds autosave (F-567, F-584); restore rules are authored in `experimental::restore_rules` (F-663); the preserved server loads, saves and reloads a supplied copy offline, dropping only two physics-dynamic tree records and one fully mined respawned voxel record, as client validation predicts (F-678 to F-681); an interpreted import stays gated (U-045).
6. Reliability: 72-hour schedules pass in compressed time (F-615, F-616); paced wall-clock effects and physical power loss remain (U-035), and none of it is ICARUS performance (F-485).
7. Replication: spawn records, content blocks, property streams, native structs, Text, enum widths, field headers, RPCs and fast arrays are read and re-encoded bit-identically (F-569 to F-577, F-581, F-586, F-588, F-589, F-591 to F-593, F-595, F-597, F-599, F-610, F-673 to F-675, F-632, F-639, F-644); layouts and tag tables need your call (U-064, U-065, U-066).
8. Connected scenarios: this host fetches and runs the matched official server, which registers with Steam (F-687), and the release pin tracks the weekly builds whose server code changes (F-686, [architecture](architecture.md#release-tracking)); scenario S1 awaits the user's join (U-047).
