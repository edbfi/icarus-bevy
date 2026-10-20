# Actor channel bunches

## Status

Experimental: static observations of client build 25793767 and server build
25621760, wire-checked against the 3.0.31 capture (F-704 to F-711, F-739,
F-761). Last reconciled: 2026-10-21.

## Scope

Actor-channel framing and scheduling after the control channel: exports,
partial bunches, spawn records, content blocks, close and dormancy, and static
server relevancy, priority, tick-rate, readiness and consider-list policy.
Address pairs are server / client.

## Confirmed

- NetGUID references and export flags: F-572; checksum placement on the wire:
  F-692. Object exports carry the class network checksum of F-776
  (`experimental::class_checksum`); package and level exports carry `0`
  (F-777, F-778).
- Gameplay-tag net index (F-702; U-065 resolved): `ConstructGameplayTagTree`
  builds the tree; `ConstructNetIndex` sorts nodes
  by `GetAllNodes` + `FName::LexicalLess`, sets `InvalidTagNetIndex = N + 1`,
  `total_bits = 12` and clamped `first_segment = 12`, and logs
  `NetworkGameplayTagNodeIndexHash`, a chained CRC-32 of lowercase UTF-16 names
  (`experimental::tag_dictionary`).
- Exports, partial bunches and must-be-mapped prefixes (F-709 to F-711):
  `UChannel::SendBunch` (`0x14353c300` / `0x1437c9690`; `UActorChannel::SendBunch`
  `0x143522160` / `0x1437b2680`) caps at
  `NetMaxConstructedPartialBunchSizeBytes`, uses `MAX_SINGLE_BUNCH_SIZE_BITS = 7,625` and
  `MAX_PARTIAL_BUNCH_SIZE_BITS = 7,624` (`953` B for `MaxPacket = 1024`,
  `MaxPacketHandlerBits = 2`), calls `AppendExportBunches` (`0x14351fe30` /
  `0x1437b03d0`, with F-711's pair) and `AppendMustBeMappedGuids`, and marks
  `bPartialInitial` + `bOpen` on part 0 and `bPartialFinal` + `bClose` on the last
  (`experimental::sequencing`, `experimental::actor_channel`).
  `ReceivedRawBunch` (`0x143538460` / `0x1437c5cc0`) runs `ReceiveNetGUIDBunch`
  (`0x143719700` / `0x1439b8bb0`) on `bHasPackageMapExports` before reliable
  ordering (`InRec`). Export-only bunches (`ExportNetGUIDHeader` `0x1437089e0` /
  `0x1439ab670`, `ExportNetGUID` `0x143707e50` / `0x1439aabb0`) carry
  `bHasRepLayoutExport` (`0` for GUID exports), the count and
  `InternalLoadObject` (`0x1439af1c0`) references. `ReceivedNextBunch`
  (`0x143537690` / `0x1437c5500`) reassembles into `InPartialBunch`, and
  `UActorChannel::ReceivedBunch` (`0x143535c60` / `0x1437c43f0`) reads
  `bHasMustBeMappedGUIDs` before `ProcessBunch` (`0x143530b30` / `0x1437c0c30`).
- Open bunch, actor hook and dormancy (F-569, F-610, F-709 to F-711, F-739,
  F-751): `UPackageMapClient::SerializeNewActor` frames F-569's fields with
  `bQuantized = 1` for Location/Velocity and `0` for Scale3D (client
  `SerializeOptionalValue<FVector>` `0x14399cac0` and `FRotator::NetSerialize`
  `0x141ed36b0`; `SerializeCompressedShort` `0x141eeb7d0` / `0x141ed7810`).
  `ProcessBunch` and `UActorChannel::ReplicateActor` (`0x14353a820` /
  `0x1437c7f00`, `evaluate_actor_channel_replicate_actor`) call
  `OnActorChannelOpen` / `OnSerializeNewActor`: empty on `AActor` (`0x1408c9710`),
  1 byte (`uint8 NetPlayerIndex`, `+0x3d4`) on `APlayerController` (`0x1438384d0` /
  `0x143ae73a0`, `0x143838b70` / `0x143ae79d0`).
  - `ReplicateActor` exits early on `Closing && ResendAllDataState == 0`,
    `bIsReplicatingActor` (`+0x88 & 0x4`), `bActorIsPendingKill` (`+0x88 & 0x20`),
    `RF_PendingKill | RF_Unreachable` in `GUObjectArray` (sets `+0x88 |= 0x20`,
    releases `ActorReplicator` `+0x90`), an opened `OpenPacketId.First != -1`
    with ready levels and `vtable[+0x300]()`, or `FOutBunch::IsError()`.
  - It sets `FReplicationFlags` (`bNetOwner`; `bNetInitial` when
    `OpenPacketId.First == -1 || ResendAllDataState != 0` unless
    `ResendAllDataState == 2 && bSpawnedForReplay` `+0x30 & 0x2000`;
    `bNetSimulated`, `bRepPhysics`, `bReplay`), downgrades `RemoteRole`
    `ROLE_AutonomousProxy (2)` to `ROLE_SimulatedProxy (1)` while `!bNetOwner`,
    calls `SerializeNewActor`/`OnSerializeNewActor` when
    `bNetInitial && SpawnAcked` (`+0x30 & 0x1000`),
    runs `ReplicateProperties`, `ReplicateSubobjects` and stale-subobject
    deletion headers (`ResendAllDataState == 0`), sends iff bits were written,
    and always sets `LastUpdateTime = Driver->ElapsedTime` (`+0x80`) and clears
    `bForceCompareProperties` (`+0x88 &= ~2`).
  - `StartBecomingDormant` (`0x1435419d0` / `0x1437ce5b0`) sets
    `bPendingDormancy = 1` across `ReplicationMap` (`+0xa0`); on `bClose` with
    `CloseReason == 1` (`ChannelCloseReason::Dormancy`), `UActorChannel::CleanUp`
    sets `Actor->NetDormancy = DORM_DormantAll` (`2`) and keeps the actor.
- Content blocks (F-571, F-576, F-581, F-586, F-609, F-644, F-703 to F-709):
  framing per F-576 and F-581; the payload feeds
  `FObjectReplicator::ReplicateProperties` (`0x1437f6b80`).
  `FRepLayout::ReceiveProperties` (`0x143b20720`) reads `bDoChecksum` and the
  F-571 handle stream, then field headers (`SerializeInt` index below
  `FClassNetCache::GetMaxIndex() + 1` `0x1437c2650`, packed bit count, payload;
  `experimental::rep_rpc`). Handle order follows F-609 (native `ConstructUClass`
  descriptors; cooked `UBlueprintGeneratedClass` exports keep `ChildProperties`
  order; `experimental::rep_layout`: 64 schemas).
  Custom-delta fast arrays carry a 1-bit header, packed static-array index, four
  4-byte header words, 4-byte delete IDs and changed elements.
- Spawn vectors and property serializers (F-571, F-573 to F-575, F-583, F-588,
  F-589, F-591 to F-593, F-595, F-597, F-599, F-632, F-639, F-673 to F-675,
  F-704 to F-709, F-739, F-762; U-064, U-066 resolved):
  - Spawn vectors and compressed rotators: F-571, F-573 (quantized bit count
    `<= 24`).
  - `FRepLayout::ReceiveCustomProperty` (`0x143b219b0`) dispatches the F-574 and F-632 item
    types (`_MAX = max_explicit + 1`), `FNumericProperty` (1/2/4/8 B), 12-bit
    gameplay tags, network Names, archive Strings and soft-object paths, and v522 `FText`
    histories (`experimental::rep_text`). Structs (`StructFlags & 0x400`): `3,637`
    of `3,960` `TCppStructOps` return `0` at `0x1408cb520` and recurse via
    `0x143af7340`; `323` define `NetSerialize`, incl. `293` typed `F*RowHandle`,
    `FRowHandle`, `FVector`, `FVector2D`, `FQuat`, `FPlane`, `FDateTime`,
    `FTimespan`, `FRotator`, `FUniqueNetIdRepl`, `FStatPairRepState`,
    `FVector_NetQuantize*`, `FRepMovement` and `FHitResult`.
- Server relevancy, priority, tick rate and replication stages match across
  builds (F-761; steps F-745, F-748, F-749, F-751, which carry the server
  addresses; client addresses below):
  - `IsNetRelevantFor` on `AActor` (`0x1435d5be0`), `APawn` (`0x143a681d0`),
    `APlayerController` (`0x1434e9040`) and `AOnlineBeacon` (`0x1409cc210`:
    `GetNetConnection` equality when `bOnlyRelevantToOwner`, else
    `Class == RealViewer->Class`), and `IsWithinNetRelevancyDistance`
    (`0x1435d6240`; `evaluate_*_net_relevancy`).
  - `GetNetPriority` on `AActor` (`0x1435d14e0`; the `4.0`
    `ViewTarget`/`Instigator` boost precedes the `!bHidden && RootComponent`
    spatial block), `APlayerController` (`0x143adfc40`) and `AMatineeActor`
    (`0x143b3f6c0`; adds `1.0 - CreationTime / WorldTime` when
    `InChannel == nullptr`; `evaluate_*_net_priority`).
  - `UGameEngine::GetMaxTickRate` (`0x143836250`) clamps `NetServerMaxTickRate`
    to `[1, 1000]` before `0x143993060` / `0x143c5d520`. `DefaultEngine.ini`:
    `NetServerMaxTickRate=30`, `MaxClientRate=750000`,
    `MaxInternetClientRate=750000`, `ConfiguredInternetSpeed=640000`,
    `ConfiguredLanSpeed=912000`, `ConnectionTimeout=360.0`,
    `InitialConnectTimeout=360.0`, `ResolutionConnectionTimeout=120.0`.
  - `UNetConnection::IsNetReady` (`0x143991d80`, `evaluate_connection_is_net_ready`):
    `Saturate` sets `QueuedBits = -SendBuffer.GetNumBits()`; ready iff
    `QueuedBits + SendBuffer.GetNumBits() <= 0` or within the adaptive `+0x1ba0`
    byte/packet budget. `UChannel::IsNetReady` (`0x1437bd690`,
    `evaluate_channel_is_net_ready`): `NumOutRec < 255`, then the connection's.
  - `UNetDriver::ServerReplicateActors` (`0x1439bf100`,
    `evaluate_server_replicate_actors_frame`) increments `ReplicationFrame`
    (`+0x420`), calls `PrepConnections`, sets
    `ServerTickTime = (MaxTickRate <= 0.0) ? DeltaSeconds : 1.0 / MaxTickRate` and
    `bCPUSaturated = (MaxTickRate > 0.0 && DeltaSeconds > 1.2 * ServerTickTime)`,
    runs `BuildConsiderList`, then per connection `PrioritizeActors`,
    `ProcessPrioritizedActors` and inlined `MarkRelevantActors`, and rotates
    `ClientConnections` left by `NumClientsToTick` when below
    `ClientConnections.Num()`.
  - `PrepConnections` (`0x1439c0b20`, `evaluate_prep_connections_tick_budget`,
    `is_connection_ready_for_replication`) throttles `NumClientsToTick`; a
    connection is ready iff `ViewTarget != nullptr && State == USOCK_Open (3) &&
    ElapsedTime - LastReceiveTime < 1.5`.
  - `BuildConsiderList` (`0x1439c0530`, `evaluate_consider_list_actor`) removes
    `PendingKill`, `ROLE_None` and initialized `bNetStartup && DORM_Initial (4)`
    actors, skips mismatched `NetDriverName` or future `NextUpdateTime`, lerps
    `OptimalNetUpdateDelta` from `1 / NetUpdateFrequency` to
    `max(1 / MinNetUpdateFrequency, 1 / NetUpdateFrequency)` by
    `clamp((LastNetUpdateDelta - 2.0) * 0.2, 0.0, 1.0)` when
    `LastNetUpdateDelta > 2.0` (`MinNetUpdateFrequency = 2.0` when `0.0`),
    updates `NextUpdateTime`/`LastNetUpdateTime` when `!bPendingNetUpdate`,
    clears it, and calls `CallPreReplication` (`0x14332f4f0`).
  - `PrioritizeActors` (`0x1439c0e70`, `evaluate_prioritize_actor`,
    `round_net_priority_to_int`, `evaluate_destruction_info_priority`) closes
    expired `bOnlyRelevantToOwner` channels (`CloseReason = 3`), calls
    `StartBecomingDormant` on `DORM_DormantAll (2)` / `DORM_DormantPartial (3)`,
    takes `time_since_last_update` from `SpawnPrioritySeconds` without a channel,
    else `ElapsedTime - LastUpdateTime` (`0x1437246a6..0x1437247c1` /
    `0x1439c12b2..0x1439c13c6`), rounds priority via `cvtss2si` on
    `2 * (65536.0 * p) + 0.5` then `>> 1` (`0x14372482a..0x14372483b`), and
    truncates `DestroyedStartupOrDormantActors` priority via `cvttss2si`
    (`0x143724c2a`) after scaling by `0.2` (`dot < 0 && dist_sq > 4M`) or
    `0.4` (`dot < 0 && dist_sq > 250k`, or `dot >= 0 && dist_sq > 9,998,244`).
  - `ProcessPrioritizedActors` (`0x1439c1bf0`, `evaluate_process_prioritized_actor`)
    checks `IsNetReady(0)`, sends `SendDestructionInfo`, takes a `<= 1.0` s
    fast path before `IsActorRelevantToConnection`, counts an actor recently
    relevant when `ForceRelevantFrame >= Connection->LastProcessedFrame`
    (unsigned, `+0xd4` vs `+0x200`; F-772), sets
    `RelevantTime = ElapsedTime + 0.5 * SRand()`, delays unopenable `< 1.0` Hz
    actors by `0.2 * srands()`, replicates until saturation, and closes
    irrelevant or torn-off channels when
    `has_channel && (!actor_level_initialized || !actor_is_net_startup)`
    (`CloseReason` `4` on `bTearOff`, else `3`).
  - Inlined `MarkRelevantActors` (`0x1439bfa04`, `evaluate_mark_relevant_actor_step`)
    sets `bPendingNetUpdate` when `Channel != nullptr && ElapsedTime -
    Channel->RelevantTime <= 1.0` or `IsActorRelevantToConnection` (then
    `Channel->RelevantTime = ElapsedTime + 0.5 * SRand()` if a channel exists),
    and `ForceRelevantFrame = ReplicationFrame + 1` when
    `ForceRelevantFrame >= Connection->LastProcessedFrame` for every
    unprocessed entry, relevant or not, then sets `LastProcessedFrame` (F-774).

## Candidate

- `experimental::actor_channel`, `sequencing` and `rep_layout` replay the
  captured bunches (F-709 to F-711), `PackageMapState` mappings and
  must-be-mapped lists, and model the scheduling policies; fixtures and
  randomized checks: F-763, F-764 (F-577, F-604, F-673, F-739, F-761).

## Open questions

- Authenticated runtime relevancy and priority exercise (U-038).

## Evidence

- M-420 to M-437, M-438 to M-449, M-455 to M-470, M-475 to M-548, M-552 to
  M-560, M-570 to M-588, M-602 to M-605, M-614, M-615, M-620 to M-624, M-628 to
  M-635, M-707, M-708, M-710 to M-713, M-717, M-720, M-731 to M-748, M-776,
  M-777, M-782, M-794, M-795, M-797 to M-799, M-801, M-802, M-804, M-805, M-807.
