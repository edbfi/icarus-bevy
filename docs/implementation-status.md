# Implementation status

The current checkpoint: what stands, milestone state and the next tasks. Claims
cite rows in the [findings ledgers](ledgers/findings/); open questions live in
the [unknowns ledger](ledgers/unknowns.md); preconditions live only in
[gates](gates.md).

Before restructure: 96ddb0b:docs/implementation-status.md

Last reconciled: 2026-10-20

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

The native server now types every supplied recorder value losslessly and
plans imports with field-level blockers (F-766, F-767), and `run` can host an
opt-in loopback session layer tested only with authored probes (F-768).

What the foundation does is in the
[foundation digest](research/foundation/foundation.md). Research state is in the
[C001](research/save-format/c001-constructor.md),
[save-class](research/save-format/save-class-audits.md),
[control-store](research/network/control-stores.md) and
[actor-replication](research/network/actor-replication.md) digests.

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; isolated entry-map startup observed; supplied save integrity/outer format characterized; client PE/PDB identity and MSF streams verified (F-486, F-730; U-034 resolved); producing save build log pending (U-040) |
| M1: original bug reproduction | Pending | Offline startup did not reproduce reported crashes; no official bug is marked reproduced or fixed (F-217); controlled workloads still required |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; authenticated protocol, world-data and import feasibility gates remain open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.1 / Bevy 0.20.0 foundation tested; opt-in loopback session layer admits authored probes (F-768, F-770); stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Real native filesystem faults pass; real saves preserve losslessly in native generations and run through autosaves (F-567, F-584); randomized inputs never panic the readers or the peer lab, and corrupted stores load a committed snapshot or fail closed (F-604 to F-607, F-611); every supplied value types losslessly and a dry-run import plan reports field-level blockers (F-766, F-767); value meanings and migration fidelity remain unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

Offline work, in order; connected proof keeps its gates.

1. Import (U-045, U-025): every recorder value types losslessly (F-766) and the dry-run import plan blocks only on unmapped content, now split by role (F-767, F-769). Refine the unclassified names with the [restore rules](specs/save-format/restore-rules.md), define a native content catalog with independently authored world data, then write native world records behind the plan's acceptance conditions; packaged row meanings stay out of scope (F-660).
2. Player state: candidates and construction grids frame (F-668, F-643); no saved identifier or version string discriminates the producing build (F-635, F-682); write times place the save in the 3.0.29 window (F-683, F-685, F-730), and the producing server's log would confirm it (U-040).
3. Network sessions: `run` hosts an opt-in loopback session layer over the verified framing, control vocabulary and admission mirror (F-753 to F-760, F-768) and pins the Hello version to the release profile (F-770; [control negotiation](specs/network/control-negotiation.md)). Next: notify clients on shutdown and open player-controller actor channels with the qualified codecs (F-762); stock-client sessions stay gated (U-038, U-052).
4. Replication: the codecs replay M-707 completely (F-762) and server scheduling matches across builds (F-761, [actor channel](specs/network/actor-channel.md)). A native scheduling stage drives them over supplied actor positions (F-772, F-774, F-775). Next: feed its selections into actor-channel opens and property replication with the qualified codecs.
5. Reliability: 72-hour schedules pass in compressed time (F-615, F-616) and sessions do not leak over 1,000 in-process or 200 real-binary cycles (F-768, F-770). Next: paced wall-clock effects and physical power loss (U-035); none of it is ICARUS performance (F-485).
6. Connected scenarios: this host fetches and runs the matched official server (F-687), the pin tracks the weekly builds (F-700, [architecture](architecture.md#release-tracking)), and S1 on 3.0.32 verified join, drop pod, harvest/replant and byte-identical save preservation (F-701); later scenarios await approval (U-047).
