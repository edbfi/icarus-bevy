# Implementation status

The current checkpoint: what stands, milestone state and the next tasks. Claims
cite rows in the [findings ledgers](ledgers/findings/); open questions live in
the [unknowns ledger](ledgers/unknowns.md); preconditions live only in
[gates](gates.md).

Before restructure: 96ddb0b:docs/implementation-status.md

Last reconciled: 2026-10-14

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
[save-class](research/save-format/save-class-audits.md) and
[control-store](research/network/control-stores.md) digests.

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; isolated entry-map startup observed; supplied save integrity/outer format characterized; client identity checked (F-486); exact build pairing pending |
| M1: original bug reproduction | Pending | Offline startup did not reproduce reported crashes; no official bug is marked reproduced or fixed (F-217); controlled workloads still required |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; authenticated protocol, world-data and import feasibility gates remain open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.0 / Bevy 0.20.0 foundation tested; stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Real native filesystem faults and supplied envelope integrity pass; inner real-save schema and migration fidelity remain unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

Offline work, in order; connected proof keeps its gates.

1. C001 records: the static bridge is named (F-509, F-510, F-512); audit C001's four records through the shared recorder restore path (F-343, F-344) under the save-class gates, keeping runtime and register preservation conditional (U-022, U-041).
2. Player state: resolve actor, inventory, construction, tame and mission fields and confirm the reported producing build by evidence other than class registration (F-262, F-277, F-498, U-040).
3. Control stores: owner, dispatcher and its static virtual call site are identified (F-505, F-506, F-520); qualify register preservation across the constructor super calls and base origin (U-043).
4. Control stores: compare constructor metadata and instructions guided by paired-client symbols and the two named notify slots (F-479, F-480, U-043).
5. Packet framing: the client receive and bunch-send formats, integer encodings and version 17 are recorded and implemented as an experimental codec (F-519, F-521 to F-527); next the packet-header sender, server-side correspondence and the stateless handshake to data transition (F-096, U-044).
6. Transport: advance control negotiation, checksum admission and authentication evidence (F-253, F-254, U-037, U-038).
7. World data: advance independently authored world-data feasibility; validate a small import before the heavy-world workload (F-242, U-045).
8. Reliability: collect the running 72-hour synthetic soak at its end and record it against U-035; it stays synthetic, not ICARUS performance (F-485, U-035).
9. Integration: the 35 unsigned historical commits and the 250-commit PR cap need a user decision on policy, rewrite or staged integration (F-279, F-528).
