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
- Build 26613688: F-1193 retains exact controller locations, descriptor/factory
  spans and complete FFunctionParams declaration; its original evidence limits
  stand. Later selected type/construction/cache qualifications are F-1197.
- Controller (F-1197): the older trace remains 122 bytes/34 instructions/
  126 p-code operations/two opaque edges (M-1460/ev_a40bde4cc914,
  M-1461), with calls/clobbers, alias/reentry effects and successors opaque;
  its flow, whole-function and native-flow closures remain false. New listings
  cover M-1462's independent 908-byte S_GPROC32 extent through return, without
  expanding the older trace (M-1488/ev_2a71799cb7ec, M-1488/ev_601449ecca14).
  Exact forward identities and inherited type chain declare ordered
  `UFunction*&`, `FFunctionParams const&` arguments (M-1477 to M-1485;
  M-1488/ev_8018de4b3bea); no runtime object state or storage follows.
- Selected construction arguments and cached-singleton bypass are F-1197;
  creation consumes property descriptors, not source arguments/cache indices.
  The selected enum constructor is qualified below. General ABI, immutability,
  traversal direction and callee/reentry effects stay open
  (M-1488/ev_2a71799cb7ec).
- Native registration counts and initial super-function/property dimensions
  remain F-1197's physical statistics, which alone prove no runtime bound
  (F-1197; M-1488/ev_9deb9b5f9475, M-1488/ev_601449ecca14).
- On cache miss only, the consumer calls the runtime-list producer; hits
  bypass it. The producer copies inherited ClassReps, selects locally owned
  Net properties and non-overriding Net functions, and sorts local functions
  by ascending case-insensitive FName string. Properties precede functions in
  each cache layer. The selected build-26613688 public-server ancestry ends in
  `BP_IcarusPlayerControllerSurvival_C`, derived directly from the native
  survival controller. Its conditional cache has 155 native fields plus
  5 cooked properties and 22 cooked functions: 182 total, ordinary bound 183
  (parent start+own count+1). Export-group framing is separate (F-1197;
  M-1492/ev_e687899447b7, M-1492/ev_0415b46141fb,
  M-1492/ev_b571dc207b36, M-1492/ev_8b1024fed268; M-1493, M-1494).
- Under that selected ancestry and ordinary cache/header path, fields 148/149/
  150 are `ServerCancelLeaveProspectSession`, `ServerLeaveProspectSession`,
  `ServerLeaveProspectSessionWithoutSave`; 169 is `OnServer_ReturnFocus`,
  whose effects were not inspected. Producer ready/null-PropertyLink paths,
  editor/global/CDO callbacks, external mutation and initialized cache state
  remain unobserved. No live controller class/cache or completion effect for
  169 follows. Illustrative native property ordering is not newly proved;
  these function indices depend on property counts, not their permutation
  (F-1197; M-1492/ev_8b1024fed268, M-1493, M-1494).
- Selected listing verification covers 24 starts/2,834 instructions/11,427
  instruction bytes. The two reported escapes cover direct absolute jumps
  only, excluding unresolved indirect jumps; the getter target is forward
  out-of-window. No exhaustive flow, live dispatch or whole-function behavior
  follows (F-1197; M-1488/ev_601449ecca14, M-1489 to M-1491).
  New selected listings likewise leave calls/indirect dispatch and branch
  escapes open; predominantly one-byte imported function bodies do not prove
  function coverage (M-1493, M-1494).
- Normal request 149 has one `ELeaveProspectSessionType` parameter; its byte
  child is not a second argument. Current enum construction/vtable/codec
  evidence derives a 3-bit value for registered maxima 5 or 6. This does not
  fix total RPC payload size: inherited generic RPC framing does not prove the
  complete current path. Completed `ReplicatedLeftProspectSession` uses that
  enum at native +0x95b. Flattened property handle 26 is supported by
  inherited F-846/F-1120 rules, current M-1405 ancestry/type/offset equality
  and selected current command-builder evidence; it is neither RPC 26 nor a
  freshly proved entire layout (F-1197; M-1492/ev_a0a995ccf9d1,
  M-1492/ev_9867ef330866, M-1492/ev_2851c7038d64,
  M-1492/ev_8645af881a57, M-1493, M-1494).
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
  value 4 and finalises. The byte enum spans 0..5; its value codec is qualified
  above (M-1488/ev_9deb9b5f9475). The checker tests authority,
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

- F-1197: native leave integration/persistence policy; complete current-path,
  live controller/cache/dispatch, remaining layout dependencies, durability
  and stock-client completion qualification remain open.
- U-079: live visibility bindings/effects and client profile execution remain
  unqualified (F-1034, F-1042, F-1051).

- Authenticated runtime relevancy and priority exercise (U-038).

## Evidence

M-707, M-708, M-813, M-819, M-832, M-841, M-844..M-848, M-1056,
M-1126..M-1129, M-1147..M-1149, M-1197, M-1198, M-1215, M-1217,
M-1219, M-1221, M-1223 to M-1232, M-1327 to M-1330, M-1338 to M-1340, M-1354 to M-1358, M-1371 to M-1374; earlier observations: cited findings. F-1197; M-1459 to M-1462, M-1477 to M-1485, M-1488 to M-1494.
