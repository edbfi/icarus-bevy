# Actor channel bunches

## Status

Experimental: older server 25621760/client 25793767 and 3.0.31 capture
(F-704 to F-711, F-761, F-845); visibility: server 26313077 (F-1020).
Last reconciled: 2026-11-06.

## Scope

Actor framing/scheduling after control admission: exports, partials, spawn/content,
close/dormancy, relevancy/priority/readiness and visibility RPCs.
Other function bindings are in the cited finding/evidence IDs.

Retained server/client bindings from the older builds:
`AppendExportBunches` `0x14351fe30`/`0x1437b03d0`;
`ExportNetGUID` server `0x143707e50`; `InternalLoadObject` client `0x1439af1c0`;
`ProcessBunch` server `0x143530b30`; `UActorChannel::ReceivedBunch` server
`0x143535c60`; `UActorChannel::SendBunch` client `0x1437b2680`;
`StartBecomingDormant` `0x1435419d0`/`0x1437ce5b0`;
client `SerializeOptionalValue<FVector>` `0x14399cac0`, `FRotator::NetSerialize`
`0x141ed36b0`, `SerializeCompressedShort` `0x141eeb7d0`/`0x141ed7810`;
`APlayerController` server open hook `0x1438384d0`;
`FRepLayout::ReceiveCustomProperty` client `0x143b219b0`;
max-tick-rate helper `0x143993060`/`0x143c5d520`;
`CallPreReplication` server `0x14332f4f0`.

## Confirmed

- Conditional normal server 26313077 RPC82 field: one LSB-first presence bit
  precedes custom reuse, visible, PackageName and FileName iff reuse is clear
  (otherwise loading copies PackageName). Absent skips custom decoding and
  retains initialized storage; its contents are unqualified. The selected
  nonshared sender sets presence even for zero-looking structs; shared masks
  remain unresolved (F-1020). Qualified reader/map and engine17 names select
  packed EName index <703 or FString plus raw u32 number; no alignment or
  second generic struct bit. Live profile/layout/ops remain unproved.
- 3.0.32 PlayerData/Bestiary field bounds: 27/8; registered counts: 26/7
  (F-1015). Matched 3.0.34 controller/backend/PlayerData/Bestiary bounds:
  183/149/27/8 (F-1016); static framing only.
- NetGUID references, export flags and checksums: F-850, F-852, F-853.
  Controller/GameState open composition: F-852, F-794; four opens after Join:
  F-784, F-804.
- Authored bridge behavior and limits: F-938 and
  [bridge operations](../../runbooks/native-server.md#network-sessions-experimental).
- Dynamic GUIDs assign on first serialization, by open or reference (F-794);
  components keep one GUID across connections.
- Export repetition: F-796 (`experimental::session_open::ConnectionExports`).
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
  only for connected members); `SessionFlags` 47 (F-830);
  `LockedMissions` 50 (F-834, F-835); `bIsOpenWorldProspect` 48 and
  `bIsOutpostProspect` 49 (F-911); `RichPresenceGUID` 24 (F-912).
- Gameplay-tag net index (F-702; U-065 resolved): `ConstructGameplayTagTree`
  and `ConstructNetIndex` sort by `GetAllNodes` + `FName::LexicalLess`, set
  `InvalidTagNetIndex = N + 1`, `total_bits = 12`, clamped `first_segment = 12`,
  and log `NetworkGameplayTagNodeIndexHash` (chained lowercase UTF-16 CRC-32;
  `experimental::tag_dictionary`).
- Exports, partial bunches and must-be-mapped prefixes (F-709 to F-711):
  `UChannel::SendBunch` / `UActorChannel::SendBunch` caps at `NetMaxConstructedPartialBunchSizeBytes`,
  uses `MAX_SINGLE_BUNCH_SIZE_BITS = 7,625` and `MAX_PARTIAL_BUNCH_SIZE_BITS = 7,624`
  (`953` B at `MaxPacket = 1024`, `MaxPacketHandlerBits = 2`), calls
  `AppendExportBunches` (F-711) and
  `AppendMustBeMappedGuids`, and flags part 0 `bPartialInitial` + `bOpen`, the
  last `bPartialFinal` + `bClose` (`experimental::sequencing`, `actor_channel`).
  `ReceivedRawBunch` runs `ReceiveNetGUIDBunch` on `bHasPackageMapExports` before `InRec`
  ordering; export-only bunches (`ExportNetGUIDHeader`, `ExportNetGUID`) carry
  `bHasRepLayoutExport` (`0` for GUID exports), count and `InternalLoadObject` refs; `ReceivedNextBunch`
  reassembles `InPartialBunch`; `UActorChannel::ReceivedBunch` reads `bHasMustBeMappedGUIDs` before `ProcessBunch`.
- Open bunch, actor hook and dormancy (F-709 to F-711, F-751, F-845, F-849):
  `UPackageMapClient::SerializeNewActor` frames F-845's fields with
  `bQuantized = 1` for Location/Velocity and `0` for Scale3D (client
  `SerializeOptionalValue<FVector>`, `FRotator::NetSerialize`,
  `SerializeCompressedShort`).
  `ProcessBunch` and `UActorChannel::ReplicateActor` call
  `OnActorChannelOpen` / `OnSerializeNewActor`: empty on `AActor`,
  1-B `uint8 NetPlayerIndex` (`+0x3d4`) on `APlayerController`.
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
  - `StartBecomingDormant` sets
    `bPendingDormancy = 1` across `ReplicationMap` (`+0xa0`); `bClose` with
    `CloseReason == 1` (`ChannelCloseReason::Dormancy`) sets
    `NetDormancy = DORM_DormantAll` (`2`) in `CleanUp` and keeps the actor.
- Content blocks (F-644, F-703 to F-709, F-846, F-849): a client's sub-object
  block carries no stably-named bit or class after its reference (F-972).
  F-849 framing feeds
  `FObjectReplicator::ReplicateProperties`.
  `FRepLayout::ReceiveProperties` reads `bDoChecksum`, the F-846
  handle stream and field headers (`SerializeInt` below
  `FClassNetCache::GetMaxIndex() + 1`, packed bit count, payload;
  `experimental::rep_rpc`). Handle order follows F-846 (native `ConstructUClass`
  descriptors; cooked `UBlueprintGeneratedClass` exports keep `ChildProperties` order;
  `experimental::rep_layout`: 64 schemas). Custom-delta fast arrays carry a
  1-bit header, packed static index, four 4-B words, 4-B delete IDs and changed
  elements.
- Spawn vectors and property serializers (F-588, F-589, F-593, F-597, F-639,
  F-673 to F-675, F-704 to F-709, F-762, F-843, F-845 to F-848; U-064, U-066
  resolved): spawn vectors/rotators follow F-845 (`<= 24` quantized bits).
  `FRepLayout::ReceiveCustomProperty` dispatches F-847 item
  types (`_MAX = max_explicit + 1`), `FNumericProperty` (1/2/4/8 B), 12-bit tags,
  network Names, archive Strings/soft paths and v522 `FText` histories
  (`experimental::rep_text`). Of `3,960` `TCppStructOps` (`StructFlags & 0x400`),
  `3,637` return `0` and recurse; `323` define
  `NetSerialize` (`293` `F*RowHandle`, `FRowHandle`, `FVector`, `FVector2D`,
  `FQuat`, `FPlane`, `FDateTime`, `FTimespan`, `FRotator`, `FUniqueNetIdRepl`,
  `FStatPairRepState`, `FVector_NetQuantize*`, `FRepMovement`, `FHitResult`).
- Server relevancy, priority, tick rate and replication stages match across
  builds (F-761; server addresses in F-745, F-748, F-749, F-751; client bindings in the cited evidence):
  - `IsNetRelevantFor`: `AActor`, `APawn`,
    `APlayerController`, `AOnlineBeacon` (`GetNetConnection` equality if `bOnlyRelevantToOwner`, else
    `Class == RealViewer->Class`), and `IsWithinNetRelevancyDistance`.
  - `GetNetPriority`: `AActor` (`4.0` `ViewTarget`/`Instigator`
    boost precedes `!bHidden && RootComponent`), `APlayerController`, `AMatineeActor` (adds
    `1.0 - CreationTime / WorldTime` when `InChannel == nullptr`).
  - `UGameEngine::GetMaxTickRate` clamps `NetServerMaxTickRate`
    to `[1, 1000]` before the rate helper (`DefaultEngine.ini`:
    `NetServerMaxTickRate=30`, `MaxClientRate` and `MaxInternetClientRate`
    `750000`, `ConfiguredInternetSpeed=640000`, `ConfiguredLanSpeed=912000`,
    `ConnectionTimeout` and `InitialConnectTimeout` `360.0`,
    `ResolutionConnectionTimeout=120.0`).
  - `UNetConnection::IsNetReady`:
    `Saturate` sets `QueuedBits = -SendBuffer.GetNumBits()`; ready iff
    `QueuedBits + SendBuffer.GetNumBits() <= 0` or within `+0x1ba0`.
    `UChannel::IsNetReady`:
    `NumOutRec < 255` and connection ready.
  - `UNetDriver::ServerReplicateActors` increments
    `ReplicationFrame` (`+0x420`), runs `PrepConnections`, sets
    `ServerTickTime = (MaxTickRate <= 0.0) ? DeltaSeconds : 1.0 / MaxTickRate`
    and `bCPUSaturated = (MaxTickRate > 0.0 && DeltaSeconds > 1.2 * ServerTickTime)`,
    runs `BuildConsiderList`, then per connection `PrioritizeActors`,
    `ProcessPrioritizedActors` and inlined `MarkRelevantActors`, rotating
    `ClientConnections` left by `NumClientsToTick` when `< ClientConnections.Num()`.
  - `PrepConnections` throttles `NumClientsToTick`; ready iff
    `ViewTarget != nullptr && State == USOCK_Open (3) && ElapsedTime - LastReceiveTime < 1.5`.
  - `BuildConsiderList` removes
    `PendingKill`, `ROLE_None` and initialized `bNetStartup && DORM_Initial (4)`,
    skips mismatched `NetDriverName` or future `NextUpdateTime`, lerps
    `OptimalNetUpdateDelta` from `1 / NetUpdateFrequency` to
    `max(1 / MinNetUpdateFrequency, 1 / NetUpdateFrequency)` by
    `clamp((LastNetUpdateDelta - 2.0) * 0.2, 0.0, 1.0)` when `LastNetUpdateDelta > 2.0`
    (`MinNetUpdateFrequency = 2.0` if `0.0`), updates `NextUpdateTime`/`LastNetUpdateTime`
    when `!bPendingNetUpdate`, clears it and calls `CallPreReplication`.
  - `PrioritizeActors` closes
    expired `bOnlyRelevantToOwner` channels (`CloseReason = 3`), calls
    `StartBecomingDormant` on `DORM_DormantAll (2)` / `DORM_DormantPartial (3)`,
    uses `SpawnPrioritySeconds` without a channel else `ElapsedTime - LastUpdateTime`, rounds via
    `cvtss2si` on `2 * (65536.0 * p) + 0.5` then `>> 1`,
    and truncates `DestroyedStartupOrDormantActors` via `cvttss2si`
    after `0.2` (`dot < 0 && dist_sq > 4M`) or `0.4` (`dot < 0 && dist_sq > 250k`,
    or `dot >= 0 && dist_sq > 9,998,244`).
  - `ProcessPrioritizedActors`
    checks `IsNetReady(0)`, sends `SendDestructionInfo`, applies F-788's
    `RelevantTime` rule and F-851's `ForceRelevantFrame` rule, delays unopenable
    `< 1.0` Hz actors by `0.2 * srands()`, replicates until saturation, and
    closes irrelevant/torn-off channels when
    `has_channel && (!actor_level_initialized || !actor_is_net_startup)`
    (`CloseReason` `4` on `bTearOff`, else `3`).
  - Inlined `MarkRelevantActors`
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

- Modifier time (F-991, F-994, F-996): authority rejects equal/unordered
  requests. Quantize f32 `s = seconds * 10`,
  `r = round_ties_even(-0.5 - (s + s))`, then `max(0, -(i32(r) >> 1))`.
  Out-of-range integer/u16 conversion retains wire (fresh restore starts at zero).
  Accepted raw writes call OnRep even with unchanged wire. OnRep reconstructs
  raw seconds as wire times binary32 0.1 only for a present nonauthority owner,
  then requests the optional lifetime multicast. Tick's authority tag refusal
  precedes countdown; intervals consume at most one period. AutoDestroy tests
  role 3, not time. Listener effects, scheduling and refresh remain open (U-069).

- Internal-temperature setter ordering and notification boundary: F-997.
  TemperatureTick (F-999) samples only when elapsed time exceeds the interval,
  consumes one period, then adjusts internal temperature on every admitted call.
  Stat reads and notifications preserve local order. Binary32 fractional state
  uses asymmetric integer conversion; `temperature::tick_temperature` defines
  the qualified arithmetic. Initial values and callee effects remain explicit.

## Candidate

Caller-qualified server 26313077/engine 17/game 0 normal little-endian visibility
parsing stages one typed value after exact consumption (F-1021). Canonical
packed integers, strict strings and 65,536-unit cap are native subset rules.
An explicit outer-layout assertion permits Absent/Present decoding without
inferring initialized defaults. No PeerLab RPC82 dispatch or callbacks.
RPC78 validates empty parameters without timeout behavior.
Opt-in [private capture](../../runbooks/network-readiness.md#private-selected-rpc82-capture): F-1022; no decoder activation.

Native modifier and temperature updates share reliable delivery (F-995, F-998).
Explicit temperature/modifier ticks retain connection-local timing (F-1000). No automatic climate,
scheduling or stock-client qualification.

- Authored native refresh transport (F-936): `PeerLab::refresh_active_prospect`
  validates complete handles 51..67 and the current dynamic open in both byte
  orders before publishing; joined peers receive deltas after open ACK and
  future joins use the latest template. Calls remain explicit; original trigger
  timing and stock-client behavior remain unqualified (U-070, U-038).
- `experimental::actor_channel`, `sequencing` and `rep_layout` replay captured
  bunches (F-709 to F-711), `PackageMapState` mappings and scheduling policies;
  fixtures and randomized checks: F-763, F-764 (F-577, F-604, F-673, F-761,
  F-845).

## Open questions

- U-079: actual 612-bit RPC82 payload, live profile/layout/ops and
  callbacks; M-1207 retains only count, so original consumption is untested.

- Authenticated runtime relevancy and priority exercise (U-038).

## Evidence

M-707, M-708, M-813, M-819, M-832, M-841, M-844..M-848, M-1056,
M-1126..M-1129, M-1147..M-1149, M-1197, M-1198, M-1215, M-1217,
M-1219, M-1221, M-1223 to M-1232; earlier observations: cited findings.
