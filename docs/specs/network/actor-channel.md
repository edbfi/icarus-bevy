# Actor channel bunches

## Status

Experimental: static observations of client build 25793767 and server build
25621760, wire-checked against the 3.0.31 capture (F-704 to F-711, F-761,
F-845). Last reconciled: 2026-10-26.

## Scope

Actor-channel framing and scheduling after the control channel: exports,
partial bunches, spawn records, content blocks, close and dormancy, and static
server relevancy, priority, tick-rate, readiness and consider-list policy.
Address pairs are server / client.

## Confirmed

- NetGUID references, export flags and checksum placement: F-850. Object exports
  carry the F-853 class network checksum (`experimental::class_checksum`);
  package and level exports carry `0` (F-852, F-853). `experimental::session_open`
  and `game_state_open` compose the PlayerController and GameState opens
  (F-852, F-794); `run` sends the PlayerController, pawn, GameState and
  PlayerState opens after Join (F-784, F-804).
- Authored bridge (F-938; [schema-4 installation](../../runbooks/native-server.md#network-sessions-experimental)):
  motion/hidden/damage use the AActor prefix. Ordered open blocks seed known
  fields; opaque blocks invalidate them; superseded losses cannot revert
  repaired fields. Flag removal requests false/false, not class defaults.
  Saved-pose, gameplay and stock-client behavior remain unqualified.
- Dynamic GUIDs assign on first serialization, by open or reference (F-794);
  components keep one GUID across connections.
- Export repetition (F-796), server 25956137: `ShouldSendFullPath`
  `0x14372bb10`, `IsNetGUIDAcked` `0x1437181b0`, `NotifyBunchCommit`
  `0x143719b60`, `ReceivedAck` / `ReceivedNak` `0x14371e8b0` / `0x14371e9f0`
  (`experimental::session_open::ConnectionExports`).
- Actor opens with components (F-854): GUIDs and exports follow serialization
  order (actor/spawn, actor-block references by handle, component blocks,
  runtime subobject blocks naming their exported class), resolving each block's
  references after its header (`experimental::actor_open`).
- Pawn open contents (F-854): component blocks carry `bIsActive` alone or
  nothing; `UIcarusStatContainer::ReplicatedStatArray` (2) and
  `UInventory::Slots` (4) use custom-delta fast arrays with all items new
  (struct-delta bit, base key 0, no deletes, IDs from 1); empty slots send
  handles 2 and 10..18 (`experimental::pawn_open`). Vitals and inventories:
  F-836 (`network::pawn_content`); stats: F-839 (`pawn_stats`).
- GameState managers: [world managers spec](world-managers.md) (F-859, F-861,
  F-862).
- GameState open values: active prospect 51..67 and C015 handles 34, 35, 39,
  41, 42, 46 (F-855; arrays carry every element field, `IsCurrentlyPlaying`
  only for connected members; `game_state_open::active_prospect_values`,
  `ProspectInfo::for_session`); `SessionFlags` 47 (F-830;
  `network::saved_game_state`); `LockedMissions` 50 (F-834, F-835;
  `game_state_open::locked_missions_value`); `bIsOpenWorldProspect` 48 and
  `bIsOutpostProspect` 49 (F-911; `network::prospect_world_flags`);
  `RichPresenceGUID` 24 (F-912; `network::rich_presence_guid`).
- Gameplay-tag net index (F-702; U-065 resolved): `ConstructGameplayTagTree`
  and `ConstructNetIndex` sort by `GetAllNodes` + `FName::LexicalLess`, set
  `InvalidTagNetIndex = N + 1`, `total_bits = 12`, clamped `first_segment = 12`,
  and log `NetworkGameplayTagNodeIndexHash` (chained lowercase UTF-16 CRC-32;
  `experimental::tag_dictionary`).
- Exports, partial bunches and must-be-mapped prefixes (F-709 to F-711):
  `UChannel::SendBunch` (`0x14353c300` / `0x1437c9690`; `UActorChannel::SendBunch`
  `0x143522160` / `0x1437b2680`) caps at `NetMaxConstructedPartialBunchSizeBytes`,
  uses `MAX_SINGLE_BUNCH_SIZE_BITS = 7,625` and `MAX_PARTIAL_BUNCH_SIZE_BITS = 7,624`
  (`953` B at `MaxPacket = 1024`, `MaxPacketHandlerBits = 2`), calls
  `AppendExportBunches` (`0x14351fe30` / `0x1437b03d0`, with F-711's pair) and
  `AppendMustBeMappedGuids`, and flags part 0 `bPartialInitial` + `bOpen`, the
  last `bPartialFinal` + `bClose` (`experimental::sequencing`, `actor_channel`).
  `ReceivedRawBunch` (`0x143538460` / `0x1437c5cc0`) runs `ReceiveNetGUIDBunch`
  (`0x143719700` / `0x1439b8bb0`) on `bHasPackageMapExports` before `InRec`
  ordering; export-only bunches (`ExportNetGUIDHeader` `0x1437089e0` /
  `0x1439ab670`, `ExportNetGUID` `0x143707e50` / `0x1439aabb0`) carry
  `bHasRepLayoutExport` (`0` for GUID exports), count and `InternalLoadObject`
  (`0x1439af1c0`) refs; `ReceivedNextBunch` (`0x143537690` / `0x1437c5500`)
  reassembles `InPartialBunch`; `UActorChannel::ReceivedBunch` (`0x143535c60` /
  `0x1437c43f0`) reads `bHasMustBeMappedGUIDs` before `ProcessBunch`
  (`0x143530b30` / `0x1437c0c30`).
- Open bunch, actor hook and dormancy (F-709 to F-711, F-751, F-845, F-849):
  `UPackageMapClient::SerializeNewActor` frames F-845's fields with
  `bQuantized = 1` for Location/Velocity and `0` for Scale3D (client
  `SerializeOptionalValue<FVector>` `0x14399cac0`, `FRotator::NetSerialize`
  `0x141ed36b0`, `SerializeCompressedShort` `0x141eeb7d0` / `0x141ed7810`).
  `ProcessBunch` and `UActorChannel::ReplicateActor` (`0x14353a820` /
  `0x1437c7f00`, `evaluate_actor_channel_replicate_actor`) call
  `OnActorChannelOpen` / `OnSerializeNewActor`: empty on `AActor` (`0x1408c9710`),
  1-B `uint8 NetPlayerIndex` (`+0x3d4`) on `APlayerController` (`0x1438384d0` /
  `0x143ae73a0`, `0x143838b70` / `0x143ae79d0`).
  - `ReplicateActor` exits early on `Closing && ResendAllDataState == 0`,
    `bIsReplicatingActor` (`+0x88 & 0x4`), `bActorIsPendingKill` (`+0x88 & 0x20`),
    `RF_PendingKill | RF_Unreachable` in `GUObjectArray` (sets `+0x88 |= 0x20`,
    releases `ActorReplicator` `+0x90`), opened `OpenPacketId.First != -1` with
    ready levels and `vtable[+0x300]()`, or `FOutBunch::IsError()`.
  - It sets `FReplicationFlags` (`bNetOwner`; `bNetInitial` when
    `OpenPacketId.First == -1 || ResendAllDataState != 0` unless
    `ResendAllDataState == 2 && bSpawnedForReplay` `+0x30 & 0x2000`;
    `bNetSimulated`, `bRepPhysics`, `bReplay`), downgrades `RemoteRole`
    `ROLE_AutonomousProxy (2)` to `ROLE_SimulatedProxy (1)` while `!bNetOwner`,
    calls `SerializeNewActor`/`OnSerializeNewActor` on
    `bNetInitial && SpawnAcked` (`+0x30 & 0x1000`), runs `ReplicateProperties`,
    `ReplicateSubobjects` and stale-subobject deletions (`ResendAllDataState == 0`),
    sends iff bits were written, sets `LastUpdateTime = Driver->ElapsedTime`
    (`+0x80`) and clears `bForceCompareProperties` (`+0x88 &= ~2`).
  - `StartBecomingDormant` (`0x1435419d0` / `0x1437ce5b0`) sets
    `bPendingDormancy = 1` across `ReplicationMap` (`+0xa0`); `bClose` with
    `CloseReason == 1` (`ChannelCloseReason::Dormancy`) sets
    `NetDormancy = DORM_DormantAll` (`2`) in `CleanUp` and keeps the actor.
- Content blocks (F-644, F-703 to F-709, F-846, F-849): F-849 framing feeds
  `FObjectReplicator::ReplicateProperties` (`0x1437f6b80`).
  `FRepLayout::ReceiveProperties` (`0x143b20720`) reads `bDoChecksum`, the F-846
  handle stream and field headers (`SerializeInt` below
  `FClassNetCache::GetMaxIndex() + 1` `0x1437c2650`, packed bit count, payload;
  `experimental::rep_rpc`). Handle order follows F-846 (native `ConstructUClass`
  descriptors; cooked `UBlueprintGeneratedClass` exports keep `ChildProperties` order;
  `experimental::rep_layout`: 64 schemas). Custom-delta fast arrays carry a
  1-bit header, packed static index, four 4-B words, 4-B delete IDs and changed
  elements.
- Spawn vectors and property serializers (F-588, F-589, F-593, F-597, F-639,
  F-673 to F-675, F-704 to F-709, F-762, F-843, F-845 to F-848; U-064, U-066
  resolved): spawn vectors/rotators follow F-845 (`<= 24` quantized bits).
  `FRepLayout::ReceiveCustomProperty` (`0x143b219b0`) dispatches F-847 item
  types (`_MAX = max_explicit + 1`), `FNumericProperty` (1/2/4/8 B), 12-bit tags,
  network Names, archive Strings/soft paths and v522 `FText` histories
  (`experimental::rep_text`). Of `3,960` `TCppStructOps` (`StructFlags & 0x400`),
  `3,637` return `0` (`0x1408cb520`) and recurse (`0x143af7340`); `323` define
  `NetSerialize` (`293` `F*RowHandle`, `FRowHandle`, `FVector`, `FVector2D`,
  `FQuat`, `FPlane`, `FDateTime`, `FTimespan`, `FRotator`, `FUniqueNetIdRepl`,
  `FStatPairRepState`, `FVector_NetQuantize*`, `FRepMovement`, `FHitResult`).
- Server relevancy, priority, tick rate and replication stages match across
  builds (F-761; server addresses in F-745, F-748, F-749, F-751; client below):
  - `IsNetRelevantFor`: `AActor` (`0x1435d5be0`), `APawn` (`0x143a681d0`),
    `APlayerController` (`0x1434e9040`), `AOnlineBeacon` (`0x1409cc210`:
    `GetNetConnection` equality if `bOnlyRelevantToOwner`, else
    `Class == RealViewer->Class`), and `IsWithinNetRelevancyDistance`
    (`0x1435d6240`; `evaluate_*_net_relevancy`).
  - `GetNetPriority`: `AActor` (`0x1435d14e0`; `4.0` `ViewTarget`/`Instigator`
    boost precedes `!bHidden && RootComponent`), `APlayerController`
    (`0x143adfc40`), `AMatineeActor` (`0x143b3f6c0`; adds
    `1.0 - CreationTime / WorldTime` when `InChannel == nullptr`;
    `evaluate_*_net_priority`).
  - `UGameEngine::GetMaxTickRate` (`0x143836250`) clamps `NetServerMaxTickRate`
    to `[1, 1000]` before `0x143993060` / `0x143c5d520` (`DefaultEngine.ini`:
    `NetServerMaxTickRate=30`, `MaxClientRate` and `MaxInternetClientRate`
    `750000`, `ConfiguredInternetSpeed=640000`, `ConfiguredLanSpeed=912000`,
    `ConnectionTimeout` and `InitialConnectTimeout` `360.0`,
    `ResolutionConnectionTimeout=120.0`).
  - `UNetConnection::IsNetReady` (`0x143991d80`, `evaluate_connection_is_net_ready`):
    `Saturate` sets `QueuedBits = -SendBuffer.GetNumBits()`; ready iff
    `QueuedBits + SendBuffer.GetNumBits() <= 0` or within `+0x1ba0`.
    `UChannel::IsNetReady` (`0x1437bd690`, `evaluate_channel_is_net_ready`):
    `NumOutRec < 255` and connection ready.
  - `UNetDriver::ServerReplicateActors` (`0x1439bf100`,
    `evaluate_server_replicate_actors_frame`) increments `ReplicationFrame`
    (`+0x420`), runs `PrepConnections`, sets
    `ServerTickTime = (MaxTickRate <= 0.0) ? DeltaSeconds : 1.0 / MaxTickRate`
    and `bCPUSaturated = (MaxTickRate > 0.0 && DeltaSeconds > 1.2 * ServerTickTime)`,
    runs `BuildConsiderList`, then per connection `PrioritizeActors`,
    `ProcessPrioritizedActors` and inlined `MarkRelevantActors`, rotating
    `ClientConnections` left by `NumClientsToTick` when `< ClientConnections.Num()`.
  - `PrepConnections` (`0x1439c0b20`, `evaluate_prep_connections_tick_budget`,
    `is_connection_ready_for_replication`) throttles `NumClientsToTick`; ready iff
    `ViewTarget != nullptr && State == USOCK_Open (3) && ElapsedTime - LastReceiveTime < 1.5`.
  - `BuildConsiderList` (`0x1439c0530`, `evaluate_consider_list_actor`) removes
    `PendingKill`, `ROLE_None` and initialized `bNetStartup && DORM_Initial (4)`,
    skips mismatched `NetDriverName` or future `NextUpdateTime`, lerps
    `OptimalNetUpdateDelta` from `1 / NetUpdateFrequency` to
    `max(1 / MinNetUpdateFrequency, 1 / NetUpdateFrequency)` by
    `clamp((LastNetUpdateDelta - 2.0) * 0.2, 0.0, 1.0)` when `LastNetUpdateDelta > 2.0`
    (`MinNetUpdateFrequency = 2.0` if `0.0`), updates `NextUpdateTime`/`LastNetUpdateTime`
    when `!bPendingNetUpdate`, clears it and calls `CallPreReplication` (`0x14332f4f0`).
  - `PrioritizeActors` (`0x1439c0e70`, `evaluate_prioritize_actor`,
    `round_net_priority_to_int`, `evaluate_destruction_info_priority`) closes
    expired `bOnlyRelevantToOwner` channels (`CloseReason = 3`), calls
    `StartBecomingDormant` on `DORM_DormantAll (2)` / `DORM_DormantPartial (3)`,
    uses `SpawnPrioritySeconds` without a channel else `ElapsedTime - LastUpdateTime`
    (`0x1437246a6..0x1437247c1` / `0x1439c12b2..0x1439c13c6`), rounds via
    `cvtss2si` on `2 * (65536.0 * p) + 0.5` then `>> 1` (`0x14372482a..0x14372483b`),
    and truncates `DestroyedStartupOrDormantActors` via `cvttss2si` (`0x143724c2a`)
    after `0.2` (`dot < 0 && dist_sq > 4M`) or `0.4` (`dot < 0 && dist_sq > 250k`,
    or `dot >= 0 && dist_sq > 9,998,244`).
  - `ProcessPrioritizedActors` (`0x1439c1bf0`, `evaluate_process_prioritized_actor`)
    checks `IsNetReady(0)`, sends `SendDestructionInfo`, applies F-788's
    `RelevantTime` rule and F-851's `ForceRelevantFrame` rule, delays unopenable
    `< 1.0` Hz actors by `0.2 * srands()`, replicates until saturation, and
    closes irrelevant/torn-off channels when
    `has_channel && (!actor_level_initialized || !actor_is_net_startup)`
    (`CloseReason` `4` on `bTearOff`, else `3`).
  - Inlined `MarkRelevantActors` (`0x1439bfa04`, `evaluate_mark_relevant_actor_step`)
    sets `bPendingNetUpdate` when `Channel != nullptr && ElapsedTime - Channel->RelevantTime <= 1.0`
    or `IsActorRelevantToConnection` (refreshing a channel's `RelevantTime` per
    F-788), then applies F-851's `ForceRelevantFrame` and `LastProcessedFrame`
    updates.
- GameState updates (F-805, F-807): ~30 Hz unreliable actor blocks (component
  blocks may follow) carry changed `TimeOfDay` (38), 0.1 s
  `ReplicatedWorldTimeSeconds` (19), whole-second `LevelTimeElapsedSec` (46),
  2 s `ServerFPS` (82) and changed `NetworkingStatus` (68..79; F-810, F-856,
  F-808). Multi-bunch updates split as unreliable partials on consecutive
  packets (F-825; `LabPeer::actor_update_datagrams`).

## Candidate

- Authored native refresh transport (F-936): `PeerLab::refresh_active_prospect`
  validates complete handles 51..67 and the current dynamic open in both byte
  orders before publishing; joined peers receive deltas after open ACK and
  future joins use the latest template. Calls remain explicit; original trigger
  timing and stock-client behavior remain unqualified (U-070, U-038).
- `experimental::actor_channel`, `sequencing` and `rep_layout` replay captured
  bunches (F-709 to F-711), `PackageMapState` mappings/must-be-mapped lists and
  scheduling policies; fixtures and randomized checks: F-763, F-764 (F-577,
  F-604, F-673, F-761, F-845).

## Open questions

- Authenticated runtime relevancy and priority exercise (U-038).

## Evidence

- M-420 to M-437, M-438 to M-449, M-455 to M-470, M-475 to M-548, M-552 to
  M-560, M-570 to M-588, M-602 to M-605, M-614, M-615, M-620 to M-624, M-628 to
  M-635, M-707, M-708, M-710 to M-713, M-717, M-720, M-731 to M-748, M-776,
  M-777, M-782, M-794, M-795, M-797 to M-799, M-801, M-802, M-804, M-805, M-807,
  M-832, M-834 to M-836, M-841, M-844 to M-848, M-863, M-868, M-869, M-1034, M-1056.
