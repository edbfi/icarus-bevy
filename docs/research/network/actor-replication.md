# Actor replication after login

[Actor channel](../../specs/network/actor-channel.md).

## Question cluster

Actor opens, codecs, relevancy and readiness after admission.

## Current answer

Bounds/RPCs/opens F-1015/F-1016/F-1019/F-1017.
[Visibility](../../specs/network/visibility-state.md): F-1020 wire, F-1021 staging,
F-1022 capture, F-1023 conditional install/registration/logging/consumer;
F-1024 inert models; F-1034 replay. Live bindings/cause: U-079.

Dedicated state/predicate and restart producer chain: F-1036. Frozen matched
capture omitted configured startup actions (F-1037). A restart-only run observed
Begin and a UserProfile wait, without world entry (F-1038). Matched request guards
and callback routes are F-1039; diagnostic phase distinctions are F-1040. No
client handler execution or framing defect follows from static bodies.

Tags F-702/F-1031; sources F-828/F-832.
Replay F-762.

The 3.0.35 pin has qualified tags (F-1031), retained class profiles/bounds
(F-1032) and a changed row registry (F-1033). Native layouts F-1027/F-1028
remain distinct from live bindings; no client progress follows.

[World managers](../../specs/network/world-managers.md): managers, quests,
profiles/restore, modifier/temperature; automatic climate/scheduling unqualified.
Prospect refresh/initialization F-936/F-1014/F-1011/F-975; triggers U-070/F-822.
UID/membership: F-943 to F-946, F-949 to F-951; loadout/backend: F-956 to F-958.

## Open

- User-struct array checksums (U-067), runtime relevancy/priority (U-038),
  played-character values (U-069).
- Quest exclusions (U-076) and saved-container gaps/unreferenced-container
  nulling: [world-manager open questions](../../specs/network/world-managers.md#open-questions).

## Artifacts

F-762/F-1015 to F-1024/F-1027/F-1028 index artifacts; M-1299 to M-1308.
