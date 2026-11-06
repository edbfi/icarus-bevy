# Implementation status

The current checkpoint: what stands, milestone state and the next tasks. Claims
cite rows in the [findings ledgers](ledgers/findings/); open questions live in
the [unknowns ledger](ledgers/unknowns.md); preconditions live only in
[gates](gates.md).

Before restructure: 96ddb0b:docs/implementation-status.md

Last reconciled: 2026-11-04

## Summary

This is an independent native server foundation, not an ICARUS-compatible
server (F-487). Native Steam authentication and control Join are observed,
but world entry stalls (F-1009). Gameplay, world-data parity, official-save
import and release reliability/performance targets remain unproven.

Research continues offline and online under the user's standing instruction
(F-968): original-server runs and connected stock-client scenarios need their
own case, not a separate approval ([gates](gates.md)). Input parity: the native
server needs no more than the official dedicated server needs, never the game
client (F-828). The user reports the local client/save pair compatible (F-421).

The native server types every supplied recorder value losslessly and plans
imports with field-level blockers by role (F-766, F-898); `run` hosts an
opt-in loopback session layer tested only with authored probes (F-875, F-833).
`install-official-data` fetches the official server's data tables anonymously
without steamcmd (F-831), and replicated row handles take their indices from
them (F-830).

What the foundation does is in the
[foundation digest](research/foundation/foundation.md). Research state is in the
[C001](research/save-format/c001-constructor.md),
[save-class](research/save-format/save-class-audits.md),
[control-store](research/network/control-stores.md) and
[actor-replication](research/network/actor-replication.md) digests.

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; isolated entry-map startup observed; supplied save integrity/outer format characterized; server/client PE/PDB identities and MSF streams verified (F-878; U-034 resolved); producing save build log pending (U-040) |
| M1: original bug reproduction | Pending | Offline startup did not reproduce reported crashes; no official bug is marked reproduced or fixed (F-217); controlled workloads still required |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; the official server's data.pak fetches natively and its tables drive row handles (F-830, F-831); authenticated protocol, world-data and import feasibility gates remain open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.1 / Bevy 0.20.1 foundation tested; opt-in loopback session layer admits authored probes (F-875); stock-client slice pending M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Real native filesystem faults pass; real saves preserve losslessly in native generations and run through autosaves (F-567, F-873); randomized inputs never panic the readers or the peer lab, and corrupted stores load a committed snapshot or fail closed (F-604 to F-607, F-611); every supplied value types losslessly and a dry-run import plan reports field-level blockers (F-766, F-898); value meanings and migration fidelity remain unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

Offline work, in order; connected proof keeps its gates.

1. Data sources F-828/F-829/F-831/F-832/F-993 (U-072/U-073 resolved); saved rows F-861/F-866. Next: independent gameplay coverage/reference qualification; no client dependency or packaged gameplay values.
2. Import (U-045, U-025): typing/coverage is not restoration (F-766, F-916). Bool/I32 candidates F-920/F-927/F-929; lifecycle F-930; removal/comparator F-937/F-940; authored timer persistence F-931. Next: overrides, owner/row/subclass bindings and coordinates before all-or-nothing import. `import-save` stays closed; coverage grants no publication; packaged row meanings unsupported (F-660). [Procedures](runbooks/authored-worlds.md); costs F-926.
3. Player state: construction grids F-668/F-643; producing-build ambiguity F-635/F-896, 3.0.29 time window F-685/F-878. Next: producing server log (U-040).
4. Network: session bounds F-841/F-975/F-1015/F-1016; restore inputs F-973/F-976 to F-993; explicit modifier/temperature publication and ticks F-995 to F-1000 ([digest](research/network/actor-replication.md)). Next: environmental inputs, initial timing and callback effects before automatic ticking (U-069); new-player initialization (F-947), refresh (U-070), quests (U-076), stock-client scenarios (U-038/U-052).
5. Replication: replay/scheduling F-762/F-761, spatial lifecycle F-938, costs F-939/F-941/F-952. [Rotation contract](research/foundation/foundation.md) awaits qualification. Next: properties/saved poses (U-025), physics, velocity and stock-client behavior. [Actor channel](specs/network/actor-channel.md); [configuration](runbooks/native-server.md#network-sessions-experimental).
6. Reliability F-872/F-873/F-875/F-833/F-932/M-1055/F-877. Next: multi-day wall-clock holds with controller receipts and physical power loss (U-035); no ICARUS performance proof (F-871).
7. Connected scenarios F-1009/F-1017: [visibility](specs/network/visibility-state.md) F-1020 to F-1024 remains conditional. Next: actual payload, live bindings/consumer initialization/effects (U-079; [gates](gates.md)), then seed/FLOD/trait/RPC gaps (U-038).
