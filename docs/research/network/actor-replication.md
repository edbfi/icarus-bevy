# Actor replication after login

[Actor channel](../../specs/network/actor-channel.md).

## Question cluster

Actor opens, codecs, relevancy and readiness after admission.

## Current answer

Bounds/RPCs/opens F-1015/F-1016/F-1052/F-1034.
[Visibility](../../specs/network/visibility-state.md): F-1020 wire, F-1021 staging,
F-1022 capture, F-1023 conditional install/registration/logging/consumer;
F-1024 inert models; F-1034 replay. Bindings/cause: U-079.

State/restart F-1036; query118/callback126 F-1051; fields66/86 F-1052.
The retry finalises yet stays at Preparing geometry with no later client
calls (F-1067), so friends/finalisation is not the wait. After 126 the
original sends a connected-player record, then level actors (F-1070), but
not its static RPC113 (F-1068); native sends no record or level actors
(F-1071). Curtain inputs
F-1069; visibility bindings/effects remain U-079.

Tags F-702/F-1031; sources F-828/F-832.
Replay F-762.

Release identities/layouts F-1027/F-1028/F-1031 to F-1033 establish no live
bindings.

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

F-762/F-1015 to F-1024/F-1027/F-1028 index artifacts; M-1299 to M-1330, M-1335 to M-1337.
