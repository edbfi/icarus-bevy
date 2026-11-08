# Implementation status

Checkpoint: [findings](ledgers/findings/), [unknowns](ledgers/unknowns.md),
[preconditions](gates.md).

Before restructure: 96ddb0b:docs/implementation-status.md

Last reconciled: 2026-11-08

## Summary

Native foundation only (F-487). Steam auth/control Join observed; world entry
stalls (F-1009). Compatibility remains unproven.

Standing research authorization F-968 requires scoped cases.
Input parity F-828 forbids client dependencies. The user reports the supplied
client/save pair compatible (F-421).

Digests: [foundation](research/foundation/foundation.md) and
[replication](research/network/actor-replication.md).

## Milestones

| Milestone | Status | Evidence or next gate |
| --- | --- | --- |
| M0: reference baseline | In progress | 435 files hashed; build 25621760; entry-map startup and save outer format observed; PE/PDB/MSF identities verified (F-878; U-034 resolved); producing log U-040 pending |
| M1: original bug reproduction | Pending | No reported crash reproduced or official bug fixed (F-217); controlled workloads pending |
| M2: interoperability/data feasibility | In progress, partial observations | Stateless exchange observed; anonymous official-data fetch/table row handles F-830/F-831; protocol/world/import feasibility gates open |
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.1/Bevy 0.20.1 tested; opt-in authored loopback F-875; stock-client slice awaits M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Filesystem faults, lossless save generations/autosaves F-567/F-873; randomized readers/peer lab never panic, corrupt stores recover committed state or fail closed F-604 to F-607/F-611. Lossless typing/import blockers F-766/F-898; meanings/migration fidelity unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

1. Data F-828/F-829/F-831/F-832/F-993; saved rows F-861/F-866. Next: independent gameplay/reference qualification; no client dependency or packaged values.
2. Import U-045/U-025: restoration unproven F-766/F-916. Candidates F-920/F-927/F-929; lifecycle/removal/timers F-930/F-937/F-940/F-931. Next: overrides, owner/row/subclass bindings and coordinates before all-or-nothing import. Publication stays closed (F-660); [procedures](runbooks/authored-worlds.md), costs F-926.
3. Player state F-668/F-643; build ambiguity/time window F-635/F-896/F-685/F-878. Next: producing log U-040.
4. Network bounds F-841/F-975/F-1015/F-1016; restore F-973/F-976 to F-993; ticks/publication F-995 to F-1000. Next: environment/timing/callbacks U-069; new players F-947, refresh U-070, quests U-076, stock-client scenarios U-038/U-052.
5. Replication replay/scheduling F-762/F-761, lifecycle/costs F-938/F-939/F-941/F-952. Rotation awaits qualification. Next: properties/poses U-025, physics, velocity and stock-client behavior. [Protocol](specs/network/actor-channel.md); [configuration](runbooks/native-server.md#network-sessions-experimental).
6. Reliability F-1029/F-1030/F-872/F-873/F-875/F-833/F-932/M-1055/F-877. Next: wall-clock holds/controller receipts and physical power loss U-035; no ICARUS performance proof F-871.
7. Matched 3.0.35 profiles/tags/rows F-1031 to F-1033 and conditional visibility replay F-1034 are qualified. Restart reaches Begin but no parsed profile response was counted (F-1038). Next: private headers for actual subobject relation (F-1041/F-1042), client handler/backend execution and live visibility effects (U-038/U-079). [Visibility](specs/network/visibility-state.md); [gates](gates.md).
