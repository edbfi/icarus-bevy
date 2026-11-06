# Actor replication after login

[Actor channel](../../specs/network/actor-channel.md).

## Question cluster

Actor opens, codecs, relevancy and readiness after admission.

## Current answer

Bounds F-1015/F-1016; RPCs F-1019; opens without Begin F-1017.
[Visibility](../../specs/network/visibility-state.md): F-1020 wire, F-1021 staging,
F-1022 capture, F-1023 conditional install/registration/logging/consumer;
F-1024 inert models. Payload/live bindings: U-079; no cause proof.

Tags F-702 (U-065 resolved); server sources F-828/F-832.
F-762 replays 66,859 properties, 2,607 RPCs, 2,361 stat pairs (U-064/U-066 resolved).

[World managers](../../specs/network/world-managers.md): managers, quests,
profiles/restore, modifier/temperature; automatic climate/scheduling unqualified.
Prospect refresh/initialization:
F-936, F-1014, F-1011, F-975; original triggers remain U-070 (F-822).
UID/membership: F-943 to F-946, F-949 to F-951; loadout/backend: F-956 to F-958.

## Open

- User-struct array checksums (U-067), runtime relevancy/priority (U-038),
  played-character values (U-069).
- Quest exclusions (U-076) and saved-container gaps/unreferenced-container
  nulling: [world-manager open questions](../../specs/network/world-managers.md#open-questions).
- Prospect refresh timing U-070; callers F-822.

## Artifacts

F-762, F-1015 to F-1024 index retained artifacts; visibility: M-1207 to M-1209,
M-1211 to M-1225, M-1227 to M-1244, M-1246/M-1247.
