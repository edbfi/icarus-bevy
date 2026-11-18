# Implementation status

Checkpoint: [findings](ledgers/findings/), [unknowns](ledgers/unknowns.md),
[preconditions](gates.md).

Before restructure: 96ddb0b:docs/implementation-status.md

Last reconciled: 2026-11-18

## Summary

Native foundation with partial stock-client observations (F-487). Protected
production entry accepts Begin/profiles, finalises and releases the pawn; the
owner reports walk/look/sprint/jump with authored speed stats (F-1185). The
spawn is inside mountain geometry; collision and audio remain unqualified.
Normal leave shows saving pending and returns to gameplay; Alt+F4 ends the
session (F-1186). Foundation saves do not prove player persistence. Full
compatibility remains unproven. Checkpoint entry by an early pawn, the owner
bag projection and saved-location restore are authored and rehearsed over
loopback across a restart (F-1201, F-1202, F-1206 to F-1208). A stock client
reached finalisation and disconnected before movement release (F-1199). On
the repeat join the first own-record ACK arrived with the control close (F-1264).

Standing research authorization F-1175, renewed 2026-11-18 for offline and
online work, still requires scoped cases.
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
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.100.0/Bevy 0.20.1 tested; opt-in authored loopback F-875; stock-client slice awaits M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Filesystem faults, lossless save generations/autosaves F-567/F-873; randomized readers/peer lab never panic, corrupt stores recover committed state or fail closed F-604 to F-607/F-611. Lossless typing/import blockers F-766/F-898; meanings/migration fidelity unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

1. Data F-828/F-829/F-831/F-832/F-993; saved rows F-861/F-866. Next: independent gameplay/reference qualification; no client dependency or packaged values.
2. Import U-045/U-025: restoration unproven F-766/F-916. Candidates F-920/F-927/F-929; reuse F-1195's list/base join; lifecycle/removal/timers F-930/F-937/F-940/F-931. One historical server dispatch wrapper copies 24 bytes into a stack argument (F-1260). Next: final argument contents, remaining definitions/storage/ABI, comparator operand/receiver proof, overrides, native owner/subclass bindings and coordinates before all-or-nothing import. Publication stays closed (F-660); [procedures](runbooks/authored-worlds.md), costs F-926.
3. Player state F-668/F-643; build ambiguity/time window F-635/F-896/F-685/F-878. Next: producing log U-040. Authored coverage: checkpoints F-1172, leave F-1188, survival F-1183, bags/Use recovery and transfers F-1200, owner bag projection F-1202, saved location and its restore F-1206/F-1207 ([operations](runbooks/authored-survival.md)); next: authoritative state coverage and separately scoped client qualification.
4. Network bounds F-841/F-975/F-1015/F-1016; restore F-973/F-978 to F-993/F-1054 to F-1056/F-1063/F-1064/F-1066; ticks F-995 to F-1000. Entry F-1185; failed leave F-1186. F-1197 conditional leave codec retains inherited-layout/old-trace limits. F-1188 adds default-off private leave/restart checks; client proof remains open. Next: compare the outgoing entry sequence through the first record ACK and control close (F-1199/F-1201/F-1264), then qualify entry, leave and rejoin against the restart rehearsal F-1208; Begin limit F-1209; membership F-1171. F-1177 active; preserved-world hosting refuses; U-069/U-070/U-076 open.
5. Replication replay/scheduling F-762/F-761, lifecycle/costs F-938/F-952. Authored speed stats accompany stock-client locomotion F-1185; authoritative motion/physics, collision, audio and stat UI remain unqualified (U-025/U-080). Corrections: layout F-1204, codec F-1205. `pawn_receive` F-1203 will show which pawn fields a client inventory action sends. After checkpoint entry succeeds, qualify candidate F-1198 and operator spawn points F-1139/F-1140 after the mountain spawn, then saved-location restore F-1207 (F-1134 to F-1138/M-1374). [Protocol](specs/network/actor-channel.md); [configuration](runbooks/native-server.md#network-sessions-experimental).
6. Reliability F-1029/F-1030/F-872/F-873/F-875/F-833/F-932/M-1055/F-877. Next: wall-clock holds/controller receipts and physical power loss U-035; no ICARUS performance proof F-871.
7. Current pin/tags/profiles/rows F-1176 to F-1178; selected static semantics only. Entry blockers and pawn release F-1099 to F-1104/F-1110 to F-1148; U-079 resolved. Protected production now reaches entry with default record-ACK release and restart-pawn F-1185; no causal isolation. Next: normal leave/save completion F-1186, placement and properties/poses U-038/U-025. Visibility remains gated; the probe-switch run is not scheduled. [Gates](gates.md).
