# Actor replication after login

[Actor channel](../../specs/network/actor-channel.md).

Before restructure: 9288d61:docs/research/network/actor-replication.md

## Question cluster

Actor opens, codecs, relevancy and readiness after admission.

## Current answer

Bounds/RPCs/opens F-1015/F-1016/F-1052/F-1034; lifetime F-1182; entry F-1185.
F-1197: conditional fields 148/149/150/169, cache 182/bound 183; normal RPC
presence + three enum bits/exact consumption. Initialized absence preserves zero
but stays distinct on wire. Handle 26 inherits custom-struct rules; checksum-off/
nonshared completion is 20 property/30 actor-content bits (M-1498 to M-1501).
Old trace/F-1193/F-1177 limits stand; live class/cache/archive/OnRep, full layout,
durability and client result stay open. Encoding permits a default-off experiment.

[Visibility](../../specs/network/visibility-state.md): wire/staging/capture,
conditional consumer and inert models F-1020 to F-1024; replay F-1034;
payload/receiver/routing/answers F-1090 to F-1095. Native answers remain
probe-only and gated (F-1097, F-1098), unnecessary for entry (F-1148;
U-079 resolved). State/restart F-1036; query118/callback126 F-1051;
fields66/86 F-1052.

Earlier entry/frozen-state findings: F-1084/F-1085/F-1089, F-1099 to F-1113,
F-1117/F-1118/F-1121 to F-1124/F-1126. Record/streaming/startup: F-1068/F-1069,
F-1072 to F-1082/F-1086 to F-1088. Pawn release cleared the screen without an
82 answer (F-1141 to F-1148).

Moves/acks/corrections F-1129 to F-1132; placement F-1134 to F-1138/F-1149/
F-1152. Authored speed stats accompany
reported walk/look/sprint/jump after leaving a mountain spawn (F-1185);
authoritative physics, collision and audio remain unqualified. Earlier
zero-speed/provenance: F-1143/F-1147/F-1150 to F-1157/F-1164 to F-1166.
Leave/save UI failure remains F-1186. Authored leave adapter F-1188 adds no
stock-client persistence proof; production leave remains default-off.

Pin/tags/classes/row widths F-1176 to F-1178 remain static only.
Sources F-828/F-832; replay F-762.

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
- F-1197/F-1188: actual request/property/OnRep/menu behavior, player-state
  coverage, restart/rejoin and durability need scoped client qualification.
- Quest exclusions (U-076) and saved-container gaps/unreferenced-container
  nulling: [world-manager open questions](../../specs/network/world-managers.md#open-questions).

## Artifacts

F-762/F-1015 to F-1024/F-1027/F-1028 index artifacts; M-1299 to M-1330, M-1335 to M-1352, M-1354 to M-1358, M-1363 to M-1374, M-1376 to M-1382; M-1403 to M-1407; M-1415; M-1421 to M-1423. M-1459 to M-1462, M-1477 to M-1485, M-1488 to M-1494; M-1498 to M-1501.
