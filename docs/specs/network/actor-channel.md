# Actor channel bunches

## Status

Experimental: older server 25621760/client 25793767 and 3.0.31 capture
(F-704 to F-711, F-761, F-845); visibility: server 26313077 (F-1020).
Last reconciled: 2026-11-08.

## Scope

Actor framing/scheduling after control admission: exports, partials, spawn/content,
close/dormancy, relevancy/priority/readiness and visibility RPCs.
Function bindings are in the cited finding/evidence IDs; older-build
addresses are retained at `c2e76dc:docs/specs/network/actor-channel.md`.

## Confirmed

- State/predicate and possession restart chain: F-1036. Dedicated predicate
  false does not prove stock-client execution.
- Retry admits both profiles and initialises, with transport settled; friends
  and finalisation remain absent at Preparing geometry (F-1050).
- Official AddConnectedPlayer queries every registered controller via reliable
  parameterless ClientQueryForFriendsList (118); success/failure callbacks
  send ServerFriendsListUpdated (126). Native Begin queues that query,
  coalesces pressure, repairs loss and waits for an actual response (F-1051).
  ACKs alone do not establish friends readiness or UI/world progress.
- Field66 acknowledges possession; configured Backend field86 requests account
  flag synchronization through the client. Neither is a visibility ACK (F-1052).
- Matched UserProfile request/identifier/backend callback guards: F-1039.
  Public phases F-1045 and opt-in private headers F-1042 separate observation
  boundaries, not client handler execution. Matched requested subobject
  identity/outer and receiver object/outer/direction/RPC-flag gates: F-1041;
  configured identity/framing agree.
- Matched receive framing has no independent content-block cap (F-1046).
  Constructed partials use a 64 KiB executable default, overridden to 128 KiB
  by shipped configuration (F-1047). Native assembly/block envelopes follow
  that setting; fragment-count and peer budgets remain defensive (F-1043).
  CharacterProfile parameter/handler guards: F-1048; connected admission
  follows F-1050. Recognition is not independent wire identity proof.

- 3.0.35: tags F-1031, class profiles F-1032, 296-table row registry F-1033.
  Legacy row indices do not transfer.
- Visibility wire, callback/cache contract and inert model: [visibility state](visibility-state.md)
  (F-1020 to F-1024).
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
  ReplicateActor flags, lifecycle filters and dormancy: F-751/F-845/F-849.
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
  the older builds (F-761; selected server bindings F-745/F-748/F-749/F-751).
  Actor/Pawn/Controller/Beacon relevancy and Actor/Controller/Matinee priority
  remain distinct. Driver stages are PrepConnections, BuildConsiderList,
  PrioritizeActors, ProcessPrioritizedActors and inlined MarkRelevantActors.
  Readiness checks connection state, viewer, recent receive time and channel
  capacity. Consideration filters lifecycle/driver/timing state before
  CallPreReplication; prioritization handles owner expiry and dormancy.
  RelevantTime and force-frame rules remain F-788/F-851. Saturation, startup
  level initialization and tear-off affect replication/close; no runtime
  relevancy/priority qualification follows (U-038). Numeric thresholds and
  defaults remain in the source-bound constants/comments in
  [actor_channel.rs](../../../src/experimental/actor_channel.rs).
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

- U-079: live visibility bindings/effects and client profile execution remain
  unqualified; F-1034/F-1042 supply neither. Friends-query qualification
  F-1051 does not clear them.

- Authenticated runtime relevancy and priority exercise (U-038).

## Evidence

M-707, M-708, M-813, M-819, M-832, M-841, M-844..M-848, M-1056,
M-1126..M-1129, M-1147..M-1149, M-1197, M-1198, M-1215, M-1217,
M-1219, M-1221, M-1223 to M-1232, M-1327 to M-1330; earlier observations: cited findings.
