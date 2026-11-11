# Actor replication after login

[Actor channel](../../specs/network/actor-channel.md).

## Question cluster

Actor opens, codecs, relevancy and readiness after admission.

## Current answer

Bounds/RPCs/opens F-1015/F-1016/F-1052/F-1034.
[Visibility](../../specs/network/visibility-state.md): F-1020 wire, F-1021 staging,
F-1022 capture, F-1023 conditional install/registration/logging/consumer;
F-1024 inert models; F-1034 replay; payloads/bit1 F-1090, receiver/Gather
F-1091, routing F-1092; the original answers each report with two level actors
(F-1093 to F-1095); native answer probe-only, gated (F-1097, F-1098). Cause: U-079.

State/restart F-1036; query118/callback126 F-1051; fields66/86 F-1052.
Runs 1/2 finalise without tile reports (F-1084, F-1085); with controller
17/20 run 3 reports tiles, UI unchanged (F-1089). The client's blockers
clear on a fresh join (F-1101 to F-1103; the controller's inferred); the
native never unfreezes the pawn as S2 does (F-1099, F-1100); no read path
from the answer to entry (F-1104). After 126 the original sends the record
(F-1072 to F-1075), then level actors (F-1081), not RPC113 (F-1068). Clients stream
tiles (F-1078, F-1079) once their record finalises (F-1080). The native record
matches (F-1077, F-1086), but its check needs controller IcarusPlayerCharacter,
unsent until F-1088 (F-1087); 62, startup RPCs and level actors are missing
(F-1082). Curtain inputs F-1069/F-1076.

Tags F-702/F-1031; sources F-828/F-832.
Replay F-762.

[World managers](../../specs/network/world-managers.md): managers, quests,
profiles/restore, modifier/temperature; automatic climate/scheduling unqualified.
Prospect refresh/initialization F-936/F-1014/F-1011/F-975; triggers U-070/F-822.
UID/membership: F-1061, F-945, F-946, F-949 to F-951; loadout/backend: F-1062, F-958.

## Open

- User-struct array checksums (U-067), runtime relevancy/priority (U-038),
  played-character values (U-069).
- Quest exclusions (U-076) and saved-container gaps/unreferenced-container
  nulling: [world-manager open questions](../../specs/network/world-managers.md#open-questions).

## Artifacts

F-762/F-1015 to F-1024/F-1027/F-1028 index artifacts; M-1299 to M-1330, M-1335 to M-1352, M-1354, M-1355.
