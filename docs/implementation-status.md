# Implementation status

Checkpoint: [findings](ledgers/findings/), [unknowns](ledgers/unknowns.md),
[preconditions](gates.md).

Before restructure: 96ddb0b:docs/implementation-status.md

Last reconciled: 2026-11-12

## Summary

Native foundation only (F-487). Steam auth/control Join observed (F-1009).
In two bounded runs (one account) world entry no longer stalls at the loading
screen: it clears once the server releases the pawn's frozen flag (F-1148),
and the character stands on terrain at the default spawn (F-1149). It can
look and jump but not walk, and its stats read 0 (F-1143, F-1147): the
probe sent no stats, and walk speed is a stat (F-1157; U-080); move
acknowledgements change nothing (F-1150). `run` unfreezes each pawn
after its record ACK by default (F-1099), untested with a stock client.
Compatibility remains unproven.

Standing research authorization F-968, renewed 2026-11-12 for offline and
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
| M3: Bevy gameplay slice | In progress, foundation only | Rust 1.99.1/Bevy 0.20.1 tested; opt-in authored loopback F-875; stock-client slice awaits M2 |
| M4: complete behavior | Pending | All required compatibility matrix entries must pass |
| M5: migration/hardening | Pending, native hardening underway | Filesystem faults, lossless save generations/autosaves F-567/F-873; randomized readers/peer lab never panic, corrupt stores recover committed state or fail closed F-604 to F-607/F-611. Lossless typing/import blockers F-766/F-898; meanings/migration fidelity unvalidated |
| M6: optimization/qualification | Pending | Matched reference workloads and full release gates required |

## Next tasks

1. Data F-828/F-829/F-831/F-832/F-993; saved rows F-861/F-866. Next: independent gameplay/reference qualification; no client dependency or packaged values.
2. Import U-045/U-025: restoration unproven F-766/F-916. Candidates F-920/F-927/F-929; lifecycle/removal/timers F-930/F-937/F-940/F-931. Next: overrides, owner/row/subclass bindings and coordinates before all-or-nothing import. Publication stays closed (F-660); [procedures](runbooks/authored-worlds.md), costs F-926.
3. Player state F-668/F-643; build ambiguity/time window F-635/F-896/F-685/F-878. Next: producing log U-040.
4. Network bounds F-841/F-975/F-1015/F-1016; restore F-973/F-978 to F-993/F-1054 to F-1056/F-1063/F-1064/F-1066; ticks/publication F-995 to F-1000. Next: environment/timing/callbacks U-069; new players F-947, refresh U-070, quests U-076, stock-client scenarios U-038/U-052, listing password: omit L_s without one, enforce one as the original does (F-1161 to F-1163).
5. Replication replay/scheduling F-762/F-761, lifecycle/costs F-938/F-952. Rotation awaits qualification. Native acknowledges moves behind a switch (F-1127, F-1128; no visible effect with a stock client, F-1150) and takes operator spawn points (F-1139, F-1140); the default spawn lands on terrain (F-1149). Next: a probe with tables or authored movement stats, then a connected run (U-080); saved-location placement (F-1134 to F-1138, M-1374) and spawn points with a stock client. Then properties/poses U-025, physics, velocity and stock-client behavior. [Protocol](specs/network/actor-channel.md); [configuration](runbooks/native-server.md#network-sessions-experimental).
6. Reliability F-1029/F-1030/F-872/F-873/F-875/F-833/F-932/M-1055/F-877. Next: wall-clock holds/controller receipts and physical power loss U-035; no ICARUS performance proof F-871.
7. Profiles/tags/rows F-1031 to F-1033; friends query F-1051; fields66/86 F-1052; runs finalise, record ACKed, UI stalls F-1084/F-1085. Original after 126 F-1068/F-1081, record F-1072 to F-1075, curtain F-1069/F-1076, tile gate F-1078 to F-1080; native record F-1077/F-1086, possession F-1088, run 3 F-1089, gaps F-1082/F-1087. Original answer to tile reports F-1090 to F-1095; native answer probe-only F-1097/F-1098 (server refuses the key), off the read path F-1104; S2 unfreezes the pawn F-1099/F-1100, native now too (F-1141). The screen is the blocker count (F-1101, F-1117, F-1124; ends F-1102/F-1103/F-1121; logs F-1123) held by the pawn's frozen blocker (F-1126; anchor F-1110 to F-1113): runs A/B cleared it with the release alone (F-1141 to F-1148; U-079 resolved); handle 26 F-1120. Next: fresh stats for walking, five speeds suffice (F-1164 to F-1166; U-080), then the `run` binary's default release with a stock client, saved-location placement, properties/poses (U-038); the probe-switch run is not scheduled; visibility stays gated. [Gates](gates.md).
