# Actor channel bunches

## Status

Experimental: older server 25621760/client 25793767 and 3.0.31 capture
(F-704 to F-711, F-761, F-845); visibility: server 26313077 (F-1020).
Last reconciled: 2026-11-15.

## Scope

Actor framing/scheduling after control admission: exports, partials, spawn/content,
close/dormancy, relevancy/priority/readiness and visibility RPCs.
Function bindings are in the cited finding/evidence IDs.
Before restructure: c2e76dc:docs/specs/network/actor-channel.md

## Confirmed

- Connected restart/profile/backend boundaries: F-1036/F-1039/F-1041/F-1048.
  Begin queues query118; callback126 F-1051; fields66/86 F-1052 (no visibility
  ACK). Receive/content framing F-1046; constructed partial defaults/config
  64/128 KiB F-1047, followed natively F-1043. Public phase/private-header
  observation boundaries F-1045/F-1042.

- 3.0.36 pin/tags F-1176, selected class semantics F-1177 and row widths
  F-1178 remain static only. Connected behavior, level-actor identities and
  legacy row-index transfer remain unqualified.
- Build 26613688 (F-1193): 39 exact controller locations; 19 disjoint,
  file-backed, 8-byte-aligned 68-byte descriptor spans in an RW/NX section;
  19 complete 47-byte factories with direct descriptor/callee/singleton operands
  and finite caller flow (M-1459/ev_423d954d3a24, M-1459/ev_3ceb9b9b7732).
  Descriptor/singleton values, initialization, immutability, cached-path use,
  native counts/cache semantics unqualified. Complete 72-byte FFunctionParams:
  12 members/14 opaque refs (M-1459/ev_dc84ad5d8171, M-1459/ev_3bf00dd12847);
  declared offsets prove no widths, storage or entry binding. Header/source private.
- Controller (F-1197): 122 bytes/34 instructions/126 p-code
  operations/two opaque edges (M-1460/ev_a40bde4cc914). Root
  copies/loads/tests only (M-1460/M-1461); root types/entry arguments,
  calls/clobbers, alias/reentry effects and successors opaque; flow,
  whole-function and native-flow closure all false. Unique S_GPROC32 there
  declares 908 bytes, `[0x1421935d0, 0x14219395c)` (M-1462); adds no
  coverage/closure. TPI 32738: 16-byte `LF_PROCEDURE`, return 3,
  convention/options 0, count 2, arglist 32737 (M-1477/M-1478, M-1481).
  Retained 32737: 16-byte `LF_ARGLIST`, matching count 2, ordered 32735/32736
  (M-1479/M-1480, M-1481). Exact 12-byte pointer records (M-1482)
  declare attributes 65580, kind 12, mode 1, referents 8904/15137 respectively
  (M-1483; M-1481/ev_cfcf35f9a2d5). Referents: exact 12-byte records
  (M-1484; M-1481/ev_d302ebd48249). Retained: 8904 pointer attributes
  65548, kind 12, mode 0 -> opaque TPI 8902; 15137 modifier mask 1 -> opaque
  TPI 15135 (M-1485; M-1481/ev_053054206971). Grammar inherits framing;
  no original replay. Declaration codes only: no widths, names, complete
  definitions, ownership, ABI/entry-register binding or native counts/cache/wire
  149/169 proof. Return reference opaque; normal leave fails F-1186;
  F-1177 active. Future: exact TPI 8902/15135 identities
  before ABI, entry-binding and consumer/count work; both referents remain opaque.
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
- Frozen movement (F-1100): the client pawn has no movement mode while
  replicated `bServerFrozenMovement` or its own terrain-anchor flag is set;
  the native opens it true and, at F-1099's revision, never clears it.
  The client reports its own flag as pawn field 85,
  `ServerUpdateClientFrozenMovement(bool)`: reliable, a 1-bit payload,
  field-index bound 129 from the public server (F-1105, F-1106); the
  original only stores it (F-1108).
- Moves: pawn field 39 in, field 30 good-move acknowledgements out
  (F-1127, F-1128; `experimental::character_move`).
- GameState managers: [world managers spec](world-managers.md) (F-859, F-861,
  F-862).
- GameState open values: active prospect 51..67 and C015 handles 34, 35, 39,
  41, 42, 46 (F-855; arrays carry every element field, `IsCurrentlyPlaying`
  only for connected members); `SessionFlags` 47 (F-830);
  `LockedMissions` 50 (F-1060); `bIsOpenWorldProspect` 48 and
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
  ReplicateActor matches across builds (F-761); dormant CleanUp F-710;
  StartBecomingDormant and flag detail: M-748.
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
- Older-build relevancy/priority/tick/replication stages: F-761 and selected
  server bindings F-745/F-748/F-749/F-751; actor classes retain distinct
  relevancy/priority policies. Readiness, consideration and prioritization
  checks are static; RelevantTime/force-frame F-788/F-851. Saturation,
  startup-level initialization and tear-off affect replication/close;
  runtime relevancy/priority remains unqualified (U-038). Thresholds/defaults:
  [actor_channel.rs](../../../src/experimental/actor_channel.rs).
- GameState updates (F-1057, F-807): ~30 Hz unreliable actor blocks (component
  blocks may follow) carry changed `TimeOfDay` (38), 0.1 s
  `ReplicatedWorldTimeSeconds` (19), whole-second `LevelTimeElapsedSec` (46),
  2 s `ServerFPS` (82) and changed `NetworkingStatus` (68..79; F-810, F-1057,
  F-1058). Multi-bunch updates split as unreliable partials on consecutive
  packets (F-825; `LabPeer::actor_update_datagrams`).
- `ConnectedPlayers` records (F-1072 to F-1075): a reliable GameState bunch
  carries each class-named `UReplicatedConnectedPlayer` block, then handle 3
  listing their GUIDs; native: F-1077.

- Modifier time (F-991, F-994, F-996): authority rejects equal/unordered
  requests. Quantize f32 `s = seconds * 10`,
  `r = round_ties_even(-0.5 - (s + s))`, then `max(0, -(i32(r) >> 1))`.
  Out-of-range integer/u16 conversion retains wire (fresh restore starts at zero).
  OnRep: F-996; tick and AutoDestroy order: F-994. Listener effects,
  scheduling and refresh remain open (U-069).

- Internal-temperature setter ordering and notification boundary: F-997.
  TemperatureTick (F-999) samples only when elapsed time exceeds the interval,
  consumes one period, then adjusts internal temperature on every admitted call.
  Binary32 fractional state uses asymmetric integer conversion;
  `temperature::tick_temperature` defines the qualified arithmetic. Initial
  values and callee effects remain explicit.

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
- Records: staged updates as fields fill, null controller references on other
  connections, removal without a destroy (F-1073, F-1077; U-038).
- Unfreeze: S2 replicates `bServerFrozenMovement` false once, near its last
  field-82 answer and before per-frame camera updates (F-1099; one 3.0.34
  session), about 20 s after the client reported itself unfrozen (F-1107).
- After entry (S2): client moves are pawn field 39 (F-1129), answered only by
  field-30 acks and rare corrections (F-1130, F-1132); silence does not fault
  the client (F-1131). A new joiner's pawn is moved by the server before the
  unfreeze and arrives by drop pod (F-1134, F-1136); kill-Z is server-only
  (F-1137); native placement F-1138.

## Open questions

- U-079: live visibility bindings/effects and client profile execution remain
  unqualified (F-1034, F-1042, F-1051).

- Authenticated runtime relevancy and priority exercise (U-038).

## Evidence

M-707, M-708, M-813, M-819, M-832, M-841, M-844..M-848, M-1056,
M-1126..M-1129, M-1147..M-1149, M-1197, M-1198, M-1215, M-1217,
M-1219, M-1221, M-1223 to M-1232, M-1327 to M-1330, M-1338 to M-1340, M-1354 to M-1358, M-1371 to M-1374; earlier observations: cited findings. F-1197; M-1459 to M-1462, M-1477 to M-1485.
