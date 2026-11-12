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

State/restart F-1036; query118/callback126 F-1051; fields66/86 F-1052;
handle-26 width F-1120.
Runs 1/2 finalise without tile reports (F-1084, F-1085); with controller
17/20 run 3 reports tiles, UI unchanged (F-1089). Preparing geometry is the
in-between screen's fixed text, up while the viewport-blocker count is above
0 (F-1101, F-1117, F-1118, F-1124). The join-path blockers end (F-1102,
F-1103, F-1121) except the local pawn's frozen blocker, on while the pawn is
frozen (F-1126); runs 1 to 3 never unfroze it as S2 does (F-1099, F-1100).
That is the leading cause, read statically, not yet observed: after the
unfreeze the screen should hide about 2 s after the later of the server's
false and the client's valid anchor (F-1124, F-1126; anchor F-1110 to
F-1114). Client logs cannot
show blockers (F-1123); 113 is client-local (F-1122). No read path from the
answer to entry (F-1104); the client's frozen report and how to read it:
F-1105 to F-1109. After 126 the original sends the record
(F-1072 to F-1075), then level actors (F-1081), not RPC113 (F-1068). Clients stream
tiles (F-1078, F-1079) once their record finalises (F-1080). The native record
matches (F-1077, F-1086), but its check needs controller IcarusPlayerCharacter,
unsent until F-1088 (F-1087); 62, startup RPCs and level actors are missing
(F-1082). Curtain inputs F-1069/F-1076.

After entry S2 answers field-39 moves only with field-30 acks and rare
corrections (F-1129, F-1130, F-1132); unanswered moves do not fault a client
(F-1131). S2 moved its new joiner's pawn about 2.5 km before the unfreeze and
the tile reports followed that move, so only run 3 (F-1089) hints that tiles
cover the native origin (F-1114, F-1134, F-1138). The original picks a rocket
spawn (F-1136), keeps kill-Z server-side (F-1137) and saves the client's last
position (F-1135).

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

F-762/F-1015 to F-1024/F-1027/F-1028 index artifacts; M-1299 to M-1330, M-1335 to M-1352, M-1354 to M-1358, M-1363 to M-1374.
