# Recorder restoration rules

## Status

Experimental: pure rules in `experimental::restore_rules` and
`experimental::modifier_lifecycle` (`tests/save_format/restore_rules.rs`,
`tests/save_format/modifier_lifecycle.rs`, U-045; U-020..U-022, U-032, U-046,
U-048, U-050 resolved).
Last reconciled: 2026-11-19.

## Scope

Recorder validate, stitch and restore rules across all 65 `/Script/Icarus.*RecorderComponent`
classes (`C001..C038`, `N001..N027`); save decoding, class lookup, object
construction, script owners and data-table rows are out of scope (F-660).
`experimental::restore_rules` doc comments carry most addresses; unmatched
addresses retain helper names or stand alone.

## Confirmed

- `ReadFromProspectSaveState` (F-741) validates, dispatches on `+0xb2`
  (`FindOnly` `0`, `RespawnOnly` `1`, `FindOrRespawn` `2`, `ManuallyResolved`
  `3`) and then stitches: F-1210.
- Restore dispatch and ctor defaults (`C001..C038`, `N001..N027`) match across
  server 25621760 and client 25793767 (F-902, F-904, F-746, F-747): inheritance
  F-1211; `C001..C038` flags `bSaveModifiers`, `bSkipDefaultOwnerResolution`,
  `+0xb2`, `bRecordModifierStates` and `bRecordActorTransform` F-1212, F-1213;
  `+0x470`/`+0x488` F-1214 and slot overrides F-1215 (F-716); `N001..N027`
  (`N012` `UIcarusJuvenileRecorderComponent`) F-1216 to F-1218.
- `Validate` (`+0x408`; F-652, F-663, F-680, F-904, F-747): base and
  `UProspectSubsystem` membership F-1219; `C011`/`N022` reset and `N018`
  `UNoRecorderComponent` F-1220; `C025`, `C026` and `C029` F-1221; `C028`
  `"Super_Cooled_Ice"` and subclass `N020` (`UResourceDepositIceRecorderComponent`)
  F-1222 (F-645, F-902); `C031`/`C034` voxels F-1223 (F-645).
- Stitching matches GUID plus ASCII `Stricmp` (F-663) per F-1224 (`C009` F-646;
  `C018`/`C019` F-650; `C027` F-676); GUID-only matches, `C013` `FLODTiles`
  cleanup and the `C026` life restore F-1225 (F-648, F-657, F-716).
- C016 records list length and occupied positions (F-651). The server 25621760
  stitching slot selects a 2,963-byte routine (F-1266). Its loop passes a
  guarded field and a zero-based counter. Under normal Win64 register
  preservation, the model permits a skipped iteration to advance the counter
  before the first call, giving that call a nonzero index.
  The callee checks the signed index against zero and the caller’s loop bound.
  In range, it reads a 64-bit slot. For populated slots, opaque calls precede
  clearing the indexed slot. Modeled in-range normal returns carry a storage
  base. Loop-back overwrites the register with an independent load. Modeled
  exits show no explicit caller use. Cleanup calls may preserve or clobber it.
- `DoesRecorderMatchActor` (`+0x478`, F-904) per class: F-1226.
- Per-class record, restore and transfer rules for `C002`..`C006`, `C008`..`C010`,
  `C012`, `C014`, `C020`..`C024`, `C030`, `C032`, `C033`, `C035`, `C036` and `C038`
  (F-657, F-744, F-677, F-646, F-661, F-658, F-648, F-656, F-894, F-736, F-734,
  F-903, F-676) live in `experimental::restore_rules`; each function's doc
  comment carries its server / client addresses.
- C001 restoration follows F-735; C015 clocks/currencies/missions F-653;
  C037 world-boss rows F-903. Recorded level `-1` is unrecorded (F-648).
- `UIcarusGameStateRecorder` pipeline (F-750, F-902): `BeginRecording` F-1227
  to F-1229 (deferral while restoring, `RecorderComponents`, owner callbacks,
  `SpawnLocationToRecorderIndexMap`, `FlushDeferredRecorder`); `EndRecording`
  F-1230; `TryRestoreActorRecorder` and `ResolveOwnerRecorderState` F-1231;
  `HasValidOwner`, `RecordToProspectSaveState`, `RetainRecorder`,
  `PerformStitchingPass` and the `N001..N027` slot overrides F-1232.

- Saved row references and FLOD tile matching/respawn follow F-897; packaged
  row meanings remain excluded (F-660).
  Trace points: crop seeds `0x14196e790`, stomach modifiers `0x141890940`,
  FLOD name lookup `0x14189b800` and respawn `0x1418ac0f0`.
- Static-only modifier reload duration uses a distinct callback and role-`3`
  expiration wrappers issue reason-`0` `+0x3c0` (F-930, F-1233, F-1234). On
  primary `UModifierStateComponent`, matched slots, state data, stat scaling,
  scene attachment/ticks, tags/UIDs, lifetime rescaling and ordered activation
  follow F-930 and `experimental::modifier_lifecycle`
  (M-1027/ev_51aa536d68b6, M-1027/ev_c40e7a02c708,
  M-1027/ev_3681a1228c84, M-1027/ev_9785b53a7a36).
  Unresolved/non-`USceneComponent` attachment classes crash.
  Subclass/Blueprint overrides, downstream callee
  effects, owner/row binding and live execution remain unqualified.
- F-1195 declarations (server 25621760): `DataRowHandleNew`, `FRowHandle` and
  `FName`/`FNameEntryId` joins F-1235, F-1236; removal `0x1414b5950` and its
  `LF_MFUNCTION` F-1237; `UIcarusFunctionLibrary` class and `LF_ARGLIST`
  F-1238; `LF_POINTER` to forward `AActor` F-1239;
  `FModifierStatesRowHandle` forward and complete records F-1240 and its 21
  physical declarations F-1241. Framing and fixity receipts: M-1457,
  M-1458/ev_800189c44abc; M-1472 to M-1476.
  Return/this definitions, member widths/occupied storage, signatures/method effects,
  aggregate/static ABI, comparator associations,
  native owner/receiver, overrides, coordinates, lifetimes and runtime/client/import
  compatibility stay open.
- `UIcarusFunctionLibrary::RemoveModifierState` local selection (F-937) is
  F-1242; F-940's helper and entry sampling F-1243; selected call operands
  F-1244; population strides and getters F-1245; `STORE8`/`LOAD8` preservation
  F-1246. M-1508 binds static checks. M-1509/M-1510 record processes only.
- The combined search reached its match cap (M-1540). In the selected server
  dispatch wrapper, a helper copies three eight-byte values to the stack
  argument passed in RDX to modifier removal (F-1260). The post-copy callee
  has no stores or calls. On its modeled normal return with `AL=1`, removal
  receives all 24 copied bytes. With `AL=0`, two more opaque calls can change
  them. Final bytes on that path, row and actor identity, other callers, native
  storage and runtime effects remain open.

## Candidate

- Float-to-int uses x86 truncation (`i32::MIN`); trig is approximate.

## Open questions

- The exact rebind-name lookup in the selected public-symbol profile found zero
  matches (F-1266). Method, component, owner, memory aliases, outward return, saved-position
  binding and current-release restoration remain open.
- Data-table rows stay out of scope (F-660).

## Evidence

F-645..F-681, F-716, F-894, F-902..F-904, F-728, F-734..F-736, F-741..F-744, F-746, F-747,
F-750, F-897, F-930, F-937, F-940, F-1195; M-1425 to M-1428, M-1431 to M-1458, M-1471 to M-1476, M-1517; M-642..M-698, M-755..M-792, M-794..M-807, M-858, M-1024, M-1025,
M-1027, M-1028, M-1035..M-1037, M-1046..M-1048, M-1050, M-1052..M-1054, M-1058, M-1059, M-1061..M-1062, M-1065..M-1068, M-1071..M-1074.
