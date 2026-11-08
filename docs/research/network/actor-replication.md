# Actor replication after login

[Actor channel](../../specs/network/actor-channel.md).

## Question cluster

Actor opens, codecs, relevancy and readiness after admission.

## Current answer

Bounds/RPCs/opens F-1015/F-1016/F-1019/F-1017.
[Visibility](../../specs/network/visibility-state.md): F-1020 wire, F-1021 staging,
F-1022 capture, F-1023 conditional install/registration/logging/consumer;
F-1024 inert models; F-1034 replay. Bindings/cause: U-079.

State/restart F-1036; profiles and initialisation now settle (F-1050).
The wait is friends/finalisation: missing server query118, its callback126
and native request transport are F-1051. Fields66/86 are F-1052.
A scoped client retry must measure finalisation and UI progress separately;
visibility bindings/effects remain U-079.

Tags F-702/F-1031; sources F-828/F-832.
Replay F-762.

Release identities/layouts F-1027/F-1028/F-1031 to F-1033 establish no live
bindings.

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

F-762/F-1015 to F-1024/F-1027/F-1028 index artifacts; M-1299 to M-1330.
