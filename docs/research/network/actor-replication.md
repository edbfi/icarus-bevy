# Actor replication after login

[Actor channel](../../specs/network/actor-channel.md).

Before restructure: 9288d61:docs/research/network/actor-replication.md

## Question cluster

Actor opens, codecs, relevancy and readiness after admission.

## Current answer

Bounds/RPCs/opens F-1015/F-1016/F-1052/F-1034; authored controller lifetime
F-1182. Protected production entry and locomotion F-1185; normal leave fails
F-1186. Declarations F-1193/F-1196 match procedure/arglist counts, with opaque
refs and 122-byte coverage. Types/ABI, entry binding, counts/cache
and wire 149/169 stay unqualified; F-1177 stays active.

[Visibility](../../specs/network/visibility-state.md): wire/staging/capture,
conditional consumer and inert models F-1020 to F-1024; replay F-1034;
payload/receiver/routing/answers F-1090 to F-1095. Native answers remain
probe-only and gated (F-1097, F-1098), unnecessary for entry (F-1148;
U-079 resolved). State/restart F-1036; query118/callback126 F-1051;
fields66/86 F-1052; handle-26 width F-1120.

Entry blockers and earlier failed runs: F-1084/F-1085/F-1089, F-1101 to
F-1103/F-1117/F-1118/F-1121/F-1124/F-1126. Pawn release cleared the screen
without an 82 answer (F-1141 to F-1148; anchors F-1110 to F-1113).
Frozen-state evidence and reporting limits: F-1099/F-1100/F-1104 to F-1109,
F-1122/F-1123. Record, streaming and missing native startup work:
F-1068/F-1069/F-1072 to F-1082/F-1086 to F-1088.

After-entry moves/acks/corrections F-1129 to F-1132; spawn, saved position,
kill-Z and native placement F-1134 to F-1138/F-1149/F-1152. Earlier zero-speed
observations F-1143/F-1147/F-1150/F-1151; speed-stat provenance and client
units F-1153 to F-1157/F-1164 to F-1166. Authored speed stats accompany
reported walk/look/sprint/jump after leaving a mountain spawn (F-1185);
authoritative physics, collision and audio remain unqualified. Leave/save UI
failure and Alt+F4 exit remain F-1186; no player-persistence proof follows.

Current pin/tags F-1176; selected class semantics F-1177; ordered row widths
F-1178. Static only; compatibility unproven. Sources F-828/F-832; replay F-762.

[World managers](../../specs/network/world-managers.md): managers, quests,
profiles/restore, modifier/temperature; automatic climate/scheduling unqualified.
Prospect refresh/initialization F-936/F-1014/F-1011/F-975; triggers U-070/F-822.
UID/membership F-1061/F-945/F-946/F-949 to F-951; loadout/backend F-1062/F-958.

## Open

- User-struct array checksums (U-067), runtime relevancy/priority (U-038),
  played-character values (U-069).
- Stat UI and movement fidelity (U-080), spawn placement and normal leave (U-038);
  listing L_s only with
  a join password, enforced as the original does (F-1161 to F-1163).
- Quest exclusions (U-076) and saved-container gaps/unreferenced-container
  nulling: [world-manager open questions](../../specs/network/world-managers.md#open-questions).

## Artifacts

F-762/F-1015 to F-1024/F-1027/F-1028 index artifacts; M-1299 to M-1330, M-1335 to M-1352, M-1354 to M-1358, M-1363 to M-1374, M-1376 to M-1382; M-1403 to M-1407; M-1415; M-1421 to M-1423. M-1459 to M-1471, M-1477 to M-1480.
