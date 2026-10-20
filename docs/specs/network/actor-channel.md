# Actor channel bunches

## Status

Experimental: static observations of client build 25793767 and server build
25621760, wire-checked against the 3.0.31 capture (F-704 to F-711, F-739,
F-745, F-748, F-749). Last reconciled: 2026-10-20.

## Scope

How client and server frame and schedule actor-channel bunches after the control
channel: export prefixes, outgoing partial-bunch segmentation and incoming
reassembly, actor spawn records, per-object content blocks, channel close /
dormancy transitions, and static server relevancy, priority, tick-rate,
connection-readiness, consider-list, prioritization, and per-connection
replication policies.

## Confirmed

- NetGUID references (F-572, F-692): a packed GUID value (`0` null, recursion
  capped at 16); GUID 1 or an export-enabled archive adds an export-flags byte:
  bit 0 a recursive outer GUID and path string, bit 2 a 4-byte network checksum
  after the path, bit 1 no-load. Nonzero even GUIDs are dynamic.
- Gameplay-tag net-index dictionary (F-702; U-065 resolved):
  `ConstructGameplayTagTree` populates the tree from `Data/Tags/D_Tags` (130
  rows; 3 row keys != `Tag`) and `DefaultGameplayTags.ini` (1,886 tags in 3.0.31;
  1,889 in 3.0.32), expanding prefixes into 2,299 and 2,302 sorted nodes
  (`InvalidTagNetIndex = N + 1`, `total_bits = 12`, `first_segment = 12`,
  `container_bits = 6`, CRC-32 `dc780a27` / `6d0f676f`; `experimental::tag_dictionary`).
- NetGUID exports, partial-bunch split/reassembly and must-be-mapped prefixes
  (F-709 to F-711): `UChannel::SendBunch` (`0x14353c300` / `0x1437c9690`;
  `UActorChannel::SendBunch` `0x143522160` / `0x1437b2680`) enforces the 64 KiB
  partial cap, computes `MAX_SINGLE_BUNCH_SIZE_BITS = 7,625` and
  `MAX_PARTIAL_BUNCH_SIZE_BITS = 7,624` (`953` B for `MaxPacket = 1024`,
  `MaxPacketHandlerBits = 2`), calls `AppendExportBunches` (`0x143701ae0` /
  `0x1439a5290`) and `AppendMustBeMappedGuids` (`0x14351ff00` / `0x1437b0490`),
  and slices bunches `> 7,625` bits into `7,624`-bit parts (`bPartialInitial` +
  `bOpen` on 0, `bPartialFinal` + `bClose` on the last; `experimental::sequencing`
  and `experimental::actor_channel`). `ReceivedRawBunch` (`0x143538460` /
  `0x1437c5cc0`) runs `ReceiveNetGUIDBunch` (`0x143719700` / `0x1439b8bb0`) when
  `bHasPackageMapExports` is set before reliable ordering (`InRec`, max 255).
  Export-only bunches (`ExportNetGUIDHeader` `0x1437089e0`, `ExportNetGUID`
  `0x143707e50`) frame a 1-bit `bHasRepLayoutExport` flag (`0` for GUID exports),
  an `int32` count (`<= 2048`), and `InternalLoadObject` references.
  `ReceivedNextBunch` (`0x143537690`) reassembles `bPartial` bunches, and
  `UActorChannel::ReceivedBunch` (`0x143535c60`) reads `bHasMustBeMappedGUIDs`
  before `ProcessBunch` (`0x143530b30`).
- Open bunch, actor hook and dormancy transitions (F-569, F-610, F-709 to F-711,
  F-739): `UPackageMapClient::SerializeNewActor` (`0x1437201a0` / `0x1439be280`)
  frames the actor NetGUID and, for dynamic GUIDs (nonzero, even), the archetype
  NetGUID, a flag-gated level reference, and four optional presence-bit-gated
  blocks: location, rotation, scale and velocity (`SerializeOptionalValue<FVector>`
  `0x1436feda0` with `bQuantized = 1` for Location/Velocity and `0` for Scale3D,
  `Write/ReadPackedVector<10, 24>` `0x143471e90` / `0x1436fee6c`, and
  `FRotator::NetSerialize` `0x141ee74a0` -> `SerializeCompressedShort`
  `0x141eeb7d0`, F-739). `ProcessBunch` and `ReplicateActor` (`0x14353a820`) then
  invoke `OnActorChannelOpen` / `OnSerializeNewActor`: empty on `AActor`
  (`0x1408c9710`), and 1 byte (`uint8 NetPlayerIndex` at `+0x3d4`) on
  `APlayerController` (`0x1438384d0` / `0x143838b70`). `StartBecomingDormant`
  (`0x1435419d0`) sets `bPendingDormancy = 1` across `ReplicationMap` (`+0xa0`),
  and on `bClose` with `CloseReason == 1` (`ChannelCloseReason::Dormancy`,
  `136 / 136` closes in M-707), `UActorChannel::CleanUp` (`0x1435222c0`) sets
  `Actor->NetDormancy = DORM_DormantAll` (`2`) without destroying the actor.
- Content blocks and replicator loops (F-571, F-576, F-581, F-586, F-609, F-644,
  F-703 to F-709): after the open, each block has a rep-layout bit and an
  is-actor bit; a non-actor block adds the sub-object NetGUID and a stably-named
  bit, and a dynamic sub-object a class NetGUID whose zero value deletes it.
  Otherwise a packed payload-bit count feeds `FObjectReplicator::ReplicateProperties`
  (`0x1437f6b80`). `FRepLayout::ReceiveProperties` (`0x143b20720`) reads a 1-bit
  `bDoChecksum` flag and a zero-terminated packed-handle stream before the
  field-header loop (`SerializeInt` index below `FClassNetCache::GetMaxIndex() + 1`
  `0x1437c2650`, packed bit count, and payload; `experimental::rep_rpc`). Native
  `ConstructUClass` descriptors sort own `CPF_Net` properties by `(offset, name)`,
  whereas cooked `UBlueprintGeneratedClass` exports (`ClassFlags & 0x80`) keep
  `ChildProperties` order (`experimental::rep_layout`: 64 schemas covering all
  `20,407 / 20,407` `with_rep_layout` blocks in M-707). Custom-delta fast arrays
  carry a 1-bit header, packed static-array index, four 4-byte header words,
  4-byte delete IDs, and changed elements.
- Spawn vectors and property/struct serializers (F-571, F-573 to F-575, F-583,
  F-588, F-589, F-591 to F-593, F-595, F-597, F-599, F-632, F-639, F-673 to
  F-675, F-704 to F-709, F-739; U-064, U-066 resolved):
  - Spawn vectors read/write a quantized bit at `EngineNetVer >= 13`
    (unquantized: 3 floats; quantized: bit count `<= 24` and 3 integers scaled
    by `0.1`), and compressed rotators send per-axis nonzero bits and 16-bit
    angles (`182.04445f` = `65536 / 360`, F-739).
  - `FRepLayout::ReceiveCustomProperty` (`0x143b219b0`) dispatches Bool (1 bit),
    Byte/Enum (`min(8, CeilLogTwo64(max + 1))` bits with `_MAX = max_explicit + 1`),
    `FNumericProperty` (1/2/4/8 B), object NetGUIDs, structs (`StructFlags & 0x400`:
    `3,637` of `3,960` `TCppStructOps` return `0` and recurse via `0x143af7340`;
    `323` define custom `NetSerialize`, incl. `293` typed `F*RowHandle` structs,
    `FRowHandle`, `FVector`, `FVector2D`, `FQuat`, `FPlane`, `FDateTime`,
    `FTimespan`, `FRotator`, `FUniqueNetIdRepl`, `FStatPairRepState`,
    `FVector_NetQuantize*`, `FRepMovement`, `FHitResult`), 12-bit gameplay tags,
    network Names, archive Strings/soft-object paths, and v522 `FText` histories
    (`experimental::rep_text`), replaying `23,486 / 23,486` (`100%`) typed blocks
    in M-707.
- Server relevancy, priority, tick rate, and `ServerReplicateActors` stages
  (F-745, F-748, F-749):
  - `AActor::IsNetRelevantFor` (`0x14333fae0`), `APawn::IsNetRelevantFor`
    (`0x1437b95a0`), `APlayerController::IsNetRelevantFor` (`0x1432510a0`),
    `AOnlineBeacon::IsNetRelevantFor` (`0x1409a5780`),
    `IsWithinNetRelevancyDistance` (`0x143340200`), `AActor::GetNetPriority`
    (`0x14333b2e0`), `APlayerController::GetNetPriority` (`0x143830f20`), and
    `AMatineeActor::GetNetPriority` (`0x143894660`) match across both builds
    (`evaluate_*_net_relevancy`, `evaluate_*_net_priority`).
  - `UGameEngine::GetMaxTickRate` (`0x1435a90e0`, clamping `NetServerMaxTickRate`
    to `[1, 1000]`), `DefaultEngine.ini` (`NetServerMaxTickRate=30`,
    `MaxClientRate=750000`, `ConfiguredInternetSpeed=640000`,
    `ConfiguredLanSpeed=912000`, `ConnectionTimeout=360.0`,
    `ResolutionConnectionTimeout=120.0`), and `UNetDriver::ServerReplicateActors`
    (`0x1437221b0`) match: `PrepConnections` (`0x143723dc0`,
    `evaluate_prep_connections_tick_budget`, `is_connection_ready_for_replication`)
    requires `ViewTarget != nullptr && State == 3 && ElapsedTime - LastReceiveTime < 1.5`;
    `BuildConsiderList` (`0x1437235a0`, `evaluate_consider_list_actor`) removes
    `PendingKill`, `ROLE_None`, and initialized `bNetStartup && DORM_Initial (4)`
    actors and lerps `OptimalNetUpdateDelta` over `> 2.0` s (`* 0.2`,
    `MinNetUpdateFrequency = 2.0`); `PrioritizeActors` (`0x1437241e0`,
    `evaluate_prioritize_actor`, `round_net_priority_to_int`,
    `evaluate_destruction_info_priority`) closes expired `bOnlyRelevantToOwner`
    channels (`CloseReason = 3`), calls `StartBecomingDormant` on `DORM_DormantAll (2)`
    / `DORM_DormantPartial (3)`, scales priority via `appRoundToInt(65536.0 * p)`,
    and scales `DestroyedStartupOrDormantActors` by `0.2` (`dot < 0 && dist_sq > 4M`)
    or `0.4` (`dot < 0 && dist_sq > 250k`, or `dot >= 0 && dist_sq > 9,998,244`);
    `ProcessPrioritizedActors` (`0x143725030`, `evaluate_process_prioritized_actor`)
    checks `IsNetReady(0)`, sends `SendDestructionInfo`, uses a `<= 1.0` s
    fast-path before `IsActorRelevantToConnection`, updates
    `RelevantTime = ElapsedTime + 0.5 * SRand()`, delays `< 1.0` Hz unopenable
    actors by `+ 0.2 * srands()`, replicates until saturation, and closes
    irrelevant/tear-off non-startup channels (`CloseReason` `4` or `3`);
    `MarkRelevantActors` (`0x143723ce0`, `evaluate_mark_relevant_actor`) sets
    `ForceRelevantFrame = TickCount + 1` and `bPendingNetUpdate = true` when
    `Owner != nullptr && Channel == nullptr`.

## Candidate

- `experimental::actor_channel`, `experimental::sequencing` and
  `experimental::rep_layout` read, re-encode, reassemble and re-split export
  prefixes (`317 / 317`), `PackageMapState` mappings, partial-bunch sequences
  (`300 / 300`), must-be-mapped GUID lists, open bunches, content blocks,
  property streams, `ChannelCloseReason` codes (`136 / 136`), and static
  relevancy/priority/tick-rate/ServerReplicateActors policies without panicking
  on randomized inputs (F-577, F-604, F-673, F-709 to F-711, F-739, F-745,
  F-748, F-749).

## Open questions

- Authenticated runtime relevancy and priority exercise (U-038).

## Evidence

- Symbols and pseudocode M-420 to M-437; net items and writers M-438 to M-449,
  M-455 to M-470, M-776, M-777, M-782, M-794, M-795, M-797 to M-799, M-801,
  M-802; movement, tags, Name/Str, hit results, soft objects and Text M-475 to
  M-548, M-570 to M-588, M-602 to M-605; enum widths, RPCs and fast arrays
  M-614, M-615, M-620 to M-624, M-628 to M-635; native descriptors, cooked
  Blueprint exports, export/open hooks, partial/close lifecycle and wire replay
  M-552 to M-560, M-707, M-708, M-710 to M-713, M-717, M-720, M-731 to M-748.
