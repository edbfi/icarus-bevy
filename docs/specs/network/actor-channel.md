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

- Restart/profile/backend F-1036/F-1039/F-1041/F-1048; query118/callback126
  F-1051; fields66/86 F-1052 (no visibility ACK). Framing/partial bounds
  F-1043/F-1046/F-1047; observation limits F-1042/F-1045.

- 3.0.36 pin/tags F-1176, selected class semantics F-1177 and row widths
  F-1178 remain static only. Connected behavior, level-actor identities and
  legacy row-index transfer remain unqualified.
- Build 26613688 (F-1193): 39 exact controller locations; 19 disjoint,
  file-backed, 8-byte-aligned 68-byte descriptor spans in an RW/NX section;
  19 complete 47-byte factories with direct descriptor/callee/singleton operands
  and finite caller flow (M-1459/ev_423d954d3a24, M-1459/ev_3ceb9b9b7732).
  That evidence leaves values, initialization, immutability, cached-path use
  and counts/cache semantics unqualified. Complete 72-byte FFunctionParams:
  12 members/14 opaque refs (M-1459/ev_dc84ad5d8171, M-1459/ev_3bf00dd12847);
  declared offsets prove no widths, storage or entry binding. Header/source private.
- Controller (F-1197): the older trace remains 122 bytes/34 instructions/
  126 p-code operations/two opaque edges (M-1460/ev_a40bde4cc914,
  M-1461), with calls/clobbers, alias/reentry effects and successors opaque;
  its flow, whole-function and native-flow closures remain false. New listings
  cover M-1462's independent 908-byte S_GPROC32 extent through return, without
  expanding the older trace (M-1488/ev_2a71799cb7ec, M-1488/ev_601449ecca14).
  Exact TPI 8902/15135 forward identities are UFunction/FFunctionParams;
  the latter matches the retained complete definition by name, unique name,
  leaf and forward-header digest. The inherited procedure/arglist/pointer/
  modifier chain declares ordered `UFunction*&`, `FFunctionParams const&`
  arguments (M-1477 to M-1485; M-1488/ev_8018de4b3bea); no runtime object
  state or storage follows from forward declarations.
- Selected construction calls bind RCX to the singleton output reference and
  RDX to the descriptor; the consumer preserves them in R13/RSI. Optional
  factories at descriptor +0/+8 run before the singleton test. A cached
  singleton skips creation, descriptor flag/property reads and linking.
  Creation reads dword flags +64, qword size +40, qword property array +48 and
  dword count +56, sets cursor=array+8*count and calls the property constructor
  until mutable count is zero, copies words +68/+70 and links. That callee is
  unanalysed; traversal direction stays open. Count means property descriptors,
  not source arguments/cache indices.
  General ABI, immutability and callee/reentry effects stay open (F-1197;
  M-1488/ev_2a71799cb7ec).
- Seven native ancestry registrations contain 479 distinct function descriptors;
  initial Net-bit counts are 0/0/2/72/13/23/20 in their retained ancestry order,
  each with initial super-function zero. There are 25 Net-bit property
  descriptors, each dimension 1. These are physical registration statistics,
  not a runtime field bound or instantiated/cooked ancestry/order (F-1197;
  M-1488/ev_9deb9b5f9475, M-1488/ev_601449ecca14).
- On cache miss only, the cache consumer calls the unresolved runtime-list
  producer before constructing the cache; hits bypass it. It inherits parent
  start+count, appends selected properties, then functions from the runtime
  class array in its order. Ordinary field-header decoding uses
  start+own-count+1 as its integer bound, searches parent caches and slices
  the packed-bit-count payload; the export-group branch is separate. Producer
  selection, overrides, sorting and actual class ancestry remain open, so no
  concrete width or wire 149/169 binding follows (F-1197;
  M-1488/ev_7481920a03ac, M-1490, M-1491).
- Selected listing verification covers 24 starts/2,834 instructions/11,427
  instruction bytes. The two reported escapes cover direct absolute jumps
  only, excluding unresolved indirect jumps; the getter target is forward
  out-of-window. No exhaustive flow, live dispatch or whole-function behavior
  follows (F-1197; M-1488/ev_601449ecca14, M-1489 to M-1491).
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
- Component opens/GUID ordering and pawn initial arrays F-854
  (`experimental::actor_open`, `pawn_open`); vitals/inventories F-836
  (`network::pawn_content`); stats F-839 (`pawn_stats`).
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
- Gameplay-tag indices/hash F-702 (U-065 resolved): lexical ordering,
  invalid=N+1, 12-bit total/first segment; `experimental::tag_dictionary`.
- Exports, partial bunches and must-be-mapped prefixes (F-709 to F-711):
  single/partial limits 7,625/7,624 bits at packet 1024/handler 2 bits;
  constructed size has a separate cap. First/last parts carry initial+open/
  final+close. Receive processes exports before reliable ordering, reassembles
  partials, then reads must-be-mapped GUIDs before content. Export framing and
  sender ordering: F-711; `experimental::sequencing`, `actor_channel`.
- Open bunch, actor hook and dormancy (F-709 to F-711, F-751, F-845, F-849):
  location/velocity quantized, scale unquantized; actor hook empty on AActor,
  one-byte player index on APlayerController. ReplicateActor correspondence
  F-761; dormant cleanup F-710; dormancy transition/flags M-748.
- Content framing/handle order/fast arrays F-644/F-703 to F-709/F-846/F-849
  (`experimental::rep_rpc`, `rep_layout`: 64 schemas). Client subobjects omit
  stably-named bit/class after their reference (F-972). Ordinary field bounds
  depend on the actual class cache, as F-1197 qualifies above.
- Spawn vectors/rotators F-845 (at most 24 quantized bits); property dispatch,
  enum maxima, numeric widths, names/strings/text and struct serializers
  F-847/F-848 (U-064/U-066 resolved). Of 3,960 struct operations, 3,637 recurse
  and 323 serialize directly, including 293 row handles. Earlier codec
  evidence: F-588/F-589/F-593/F-597/F-639/F-673 to F-675/F-704 to F-709/
  F-762/F-843; `experimental::rep_text`.
- Older-build scheduling/relevancy F-745/F-748/F-749/F-751/F-761;
  force-frame F-788/F-851. Static readiness/priority and saturation/tear-off
  rules give no runtime qualification (U-038). Thresholds/defaults:
  [actor_channel.rs](../../../src/experimental/actor_channel.rs).
- GameState update fields/timing F-807/F-810/F-1057/F-1058; unreliable partial
  splitting F-825 (`LabPeer::actor_update_datagrams`).
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

- Normal leave (F-1197): state-notification completion candidate; no new bound
  completion RPC. The save-intending request takes one native byte; two
  descriptor properties do not mean two arguments. No-save selects declared
  value 4 and finalises. The byte enum spans 0..5; request codec/wire width
  remain unbound (M-1488/ev_9deb9b5f9475). The checker tests authority,
  cancellation and pending work. Finalisation writes the completed byte,
  emits the local event/delegate path only on change, then clears pending/
  cancel. Cancellation can retain 0 without a changed-state event. The
  selected replication notification reads that byte and calls the same path.
  Finalisation/completion descriptors lack Net. Physical vtable candidates
  prove no live dispatch; callback/timer effects, persistence policy, durable
  writes and client outcome stay open (M-1488/ev_7386f0c70678,
  M-1488/ev_7481920a03ac, M-1490, M-1491). Failed normal leave F-1186 and
  authored-only checkpoint barriers F-1188 stand; F-1177 is not superseded.

Native modifier and temperature updates share reliable delivery (F-995, F-998).
Explicit temperature/modifier ticks retain connection-local timing (F-1000). No automatic climate,
scheduling or stock-client qualification.

- Authored prospect refresh transport F-936: explicit calls; original trigger
  timing/client behavior remain unqualified (U-070/U-038).
- Replay/scheduling fixtures and randomized checks F-763/F-764; underlying
  framing F-709 to F-711. These qualify authored models only.
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

- F-1197: bind the runtime field-list producer, actual controller class ancestry
  and cache order before wire 149/169, request codec and live leave dispatch;
  then qualify persistence policy/durability and stock-client completion.
- U-079: live visibility bindings/effects and client profile execution remain
  unqualified (F-1034, F-1042, F-1051).

- Authenticated runtime relevancy and priority exercise (U-038).

## Evidence

M-707, M-708, M-813, M-819, M-832, M-841, M-844..M-848, M-1056,
M-1126..M-1129, M-1147..M-1149, M-1197, M-1198, M-1215, M-1217,
M-1219, M-1221, M-1223 to M-1232, M-1327 to M-1330, M-1338 to M-1340, M-1354 to M-1358, M-1371 to M-1374; earlier observations: cited findings. F-1197; M-1459 to M-1462, M-1477 to M-1485, M-1488 to M-1491.
