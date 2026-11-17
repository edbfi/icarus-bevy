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
- F-1197 public-server 26613688 retains F-1193's spans/declaration. Ordered
  `UFunction*&`, `FFunctionParams const&` types: M-1477 to M-1485,
  M-1488/ev_8018de4b3bea. Construction consumes descriptors; cached construction
  bypasses them. Counts alone grant no runtime bound. General ABI, traversal,
  immutability, clobber/alias/reentry remain open.
- Frozen 122-byte/34-instruction/126-p-code trace has two opaque edges and all
  three closures false (M-1460/ev_a40bde4cc914, M-1461). Independent 908-byte
  listings do not expand it (M-1462; M-1488/ev_2a71799cb7ec,
  M-1488/ev_601449ecca14). Earlier indirect edges/forward getter remain open
  (M-1488 to M-1491).
- Conditional `BP_IcarusPlayerControllerSurvival_C` inherits native survival:
  155 native + 5 cooked properties + 22 functions = 182 fields/bound 183.
  Fields 148/149/150 are cancel/normal/no-save; 169 is uninspected return-focus.
  Cache misses inherit ClassReps and append local Net properties before
  non-overriding functions sorted by folded FName. Function indices need counts,
  not a proved property permutation; export framing is separate (M-1492 to M-1494).
- Selected listings: 24 starts/2,834 instructions/11,427 instruction bytes;
  no exhaustive flow or live dispatch (M-1493, M-1494).
- Ordinary 149: one enum argument, presence bit + 3 value bits if present
  (registered maxima 5 or 6; declared byte enum 0..5);
  148/150 are empty. ReceivedRPC requires no error/exact consumption before
  dispatch; permission/hooks/GUID/reliable deferral remain
  (M-1498/ev_8fb379f7c758). Enum cast-bit 17 clear, named `memset` and RET
  initializer conditionally preserve absent zero, distinct from explicit None
  (M-1498/ev_5d6f0416550b, M-1498/ev_fe828a00c89c,
  M-1498/ev_834aa65ff472, M-1498/ev_0fd716bd216a). Enum 6/7 refusal
  is authored only.
- Completed `ReplicatedLeftProspectSession` at +0x95b is property handle 26,
  not RPC 26: inherited 15+2+2+0+6 then enum; cooked fields append. Current
  metadata/scalar emission reuse F-703/F-704/F-846/F-1120 custom-struct rules,
  not fresh mutable layout/struct-ops/CDO proof (M-1405; M-1492 to M-1494;
  M-1498/ev_0fd716bd216a).
- Checksum-off/nonshared: flag 0 + packed 26 + enum 3 bits + packed 0 = 20
  property bits; actor content adds rep-layout/actor flags + packed length = 30.
  Archive slots, packed 7-bit groups/low-bit continuation and value/copy/send
  operands support this conditional codec (M-1500/ev_f461811e1d6e,
  M-1500/ev_214a079abf4d, M-1500/ev_c47f406ef20f, M-1500/ev_1bf96fac88a2).
  Active archive/vptr/settings, zeroed backing and selected profile remain
  conditions; original canonical-varint rejection is unproved.
- M-1499's eight listings match declaration/pdata unions; M-1501 retains partial
  post-value switch, 8 padding bytes and 150-byte overflow lookahead outside
  its declaration. Calls stay opaque; one-byte imported bodies prove no coverage.
  No full callback/overflow/scheduler or live class/cache/archive proof follows.
  Producer-ready/null-PropertyLink, editor/global/CDO and mutation stay open.
  Encoding permits a default-off scoped experiment; F-1177/durability/client
  limits stand.
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

- F-1197 completion candidate: no bound completion RPC; descriptors lack Net.
  No-save selects 4/finalises. Checker tests authority/cancel/pending work.
  Finalisation writes completed state, emits local event/delegate on change,
  clears pending/cancel; Cancel may retain zero without an event. OnRep calls
  that path. Vtable candidates prove no live dispatch; callback/timer/original
  persistence/durability/client effects stay open (M-1488/ev_7386f0c70678,
  M-1488/ev_7481920a03ac, M-1490, M-1491). Failed live leave F-1186 and
  authored adapter F-1188 retain separate scopes; F-1177 stands.

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
  field-30 acks and rare corrections (F-1130, F-1132; correction layout
  F-1204, codec F-1205); silence does not fault
  the client (F-1131). A new joiner's pawn is moved by the server before the
  unfreeze and arrives by drop pod (F-1134, F-1136); kill-Z is server-only
  (F-1137); native placement F-1138.

## Open questions

- F-1197/F-1188: live class/cache/archive/OnRep/menu behavior, mutable layout,
  complete player-state coverage, restart/rejoin and original durability remain
  open. Conditional encoding permits an explicit default-off experiment only.
- U-079: live visibility bindings/effects and client profile execution remain
  unqualified (F-1034, F-1042, F-1051).

- Authenticated runtime relevancy and priority exercise (U-038).

## Evidence

M-707, M-708, M-813, M-819, M-832, M-841, M-844..M-848, M-1056,
M-1126..M-1129, M-1147..M-1149, M-1197, M-1198, M-1215, M-1217,
M-1219, M-1221, M-1223 to M-1232, M-1327 to M-1330, M-1338 to M-1340, M-1354 to M-1358, M-1371 to M-1374; earlier observations: cited findings. F-1197; M-1459 to M-1462, M-1477 to M-1485, M-1488 to M-1494; M-1498 to M-1501.
