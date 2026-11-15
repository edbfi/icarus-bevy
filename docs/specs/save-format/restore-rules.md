# Recorder restoration rules

## Status

Experimental: pure rules in `experimental::restore_rules` and
`experimental::modifier_lifecycle` (`tests/save_format/restore_rules.rs`,
`tests/save_format/modifier_lifecycle.rs`, U-045; U-020..U-022, U-032, U-046,
U-048, U-050 resolved).
Last reconciled: 2026-11-15.

## Scope

Recorder validate, stitch and restore rules across all 65 `/Script/Icarus.*RecorderComponent`
classes (`C001..C038`, `N001..N027`); save decoding, class lookup, object
construction, script owners and data-table rows are out of scope (F-660).
`experimental::restore_rules` doc comments carry most addresses; unmatched
addresses retain helper names or stand alone.

## Confirmed

- `ReadFromProspectSaveState` (F-741) runs `+0x408`, then `0x1414bee00` on
  `+0xb2`: `FindOnly` (`0`) runs `+0x450` + `0x1414b6fc0`;
  `RespawnOnly` (`1`) destroys `+0x450` actors (`0x143331760`) before `+0x458`;
  `FindOrRespawn` (`2`) tries `+0x450` then `+0x458`; `ManuallyResolved` (`3`)
  keeps the recorder; then `+0x468` runs (F-902).
- Restore dispatch and ctor defaults (`C001..C038`, `N001..N027`): all 65
  classes inherit `+0xa0 = 0x1418aaf70` and match ctor
  flags and slots `+0xa0..+0x4c0` across server 25621760 and client 25793767
  (F-902, F-904, F-746, F-747):
  - `C001..C038`: `+0xb0` (`bSaveModifiers`) `1`, but `0` on
    `C003, C007..C010, C020, C021, C025, C026, C029, C030, C033, C034`; `+0xb1`
    (`bSkipDefaultOwnerResolution`) `0`, but `1` on `C015, C020, C025, C026, C031, C034`;
    `+0xb2`: 13 `FindOnly`, 15 `RespawnOnly`, 4 `FindOrRespawn`, 6 `ManuallyResolved`;
    `+0xd8` (`bRecordModifierStates`) `1` only on `C026`; `+0xd9`
    (`bRecordActorTransform`) `1`, but `0` on `C006, C020, C028, C031, C034`;
    `+0x470`: 17 `AlwaysTrue`, `C017`
    `ParentCharacterUID != -1`, 20 `AlwaysFalse`. 31 `C001` classes inherit
    `+0x488 = 0x14189d380` (F-716); `C013`, `C014`, `C022`, `C025` and `C035..C037`
    keep the no-op. Only `C017` overrides `+0x4a0`, only
    `C004` `+0x498` (`0x14189ea40`/`0x1418a4500` vs `0x14189eae0`), and only
    `C004` (`0x14189a7d0`/`0x141888270` vs `0x14189a760`/`0x141887d20`) and `C032`
    (`0x1418ddbf0`/`0x1418c5cd0`) `+0x4a8`/`+0x4b0`.
  - `N001..N027` (non-actor `N002`, `N015`, `N018`; 24 `C001` subclasses; `N012`
    `UIcarusJuvenileRecorderComponent` appears in backup saves): `+0xb0 = 0` on
    `N001, N004, N007, N009, N012, N017..N019, N021, N022`; `+0xb1 = 1` only on
    `N021`; `+0xb2`: `FindOnly` on `N002, N003, N006, N022, N023`, `RespawnOnly`
    on `N001, N004, N007, N009..N014, N016, N017, N019, N024`, `ManuallyResolved`
    on `N018`, else `FindOrRespawn` (8); `+0xd8 = 0` on all actor classes;
    `+0xd9 = 0` only on `N020`; `+0x470`: `AlwaysTrue` on `N001, N003, N004, N007,
    N008, N016, N017, N019, N024..N027`, `CharacterParentUidPresent` on
    `N010..N014`, else `AlwaysFalse` (10); only `N011` overrides `+0x4a0`
    (`0x141400150`).
- `Validate` (`+0x408`, base `0x140899f70` = `true`; F-652, F-663, F-680, F-904, F-747):
  - `UProspectSubsystem` (`0x141704d70`, `+0x30`): the F-652 checks (via
    `0x1418231d0`) scan `0x40`-B members for `UserID` (`+0x20`, `Stricmp`) and
    `ChrSlot` (`+0x30`).
  - `C011` and `N022` (`0x1418e8790`) reset `+0x1c8`/`+0x1d8` (`0x1418ed540`) if
    `+0x1e0 < 1` or `list.rejects()`.
  - `N018` (`UNoRecorderComponent`, `0x1408cb520`)
    is always `false`.
  - `C025` (`0x1418e8850`/`0x1418f9de0`, `evaluate_player_recorder_record`)
    and `C026` (`0x1418e8b10`/`0x1418f9e80`, `evaluate_player_state_record`)
    require `v >= 1 && !list.rejects()`; `C029` (`0x1418e9280`/`0x1418f9f70`,
    `evaluate_rocket_record`) requires
    `!list.rejects()` and restores iff listed and `v > 0`.
  - `C028` (`0x1418e90b0`/`0x1418f9f20`) rejects `"Super_Cooled_Ice"`
    (`0x144a71800`);
    subclass `N020` (`UResourceDepositIceRecorderComponent`) overrides `+0x408`
    back to base `true`; `+0x460` (`0x1418f76c0`) grants stat `0xbe` iff
    `+0x1cc && !GetStat(0xbe)` (F-645, F-902).
  - `C031` and `C034` (F-680; `C034` `0x1418e96c0`) reject
    `RespawnOnly && bIsVoxelFullyMined`; `+0x460`
    (`0x1418f7bb0`) scales if `+0xb2 == 1`,
    builds a `1.1x` sphere if `+0x1d0 && !+0x1c8`, and restores voxels and `+0x45c`
    unless `+0x548 & 0x1000` (F-645).
- Stitching matches GUID plus ASCII `Stricmp` (F-663): `C009`
  (F-646); `C010` (`0x1418acbf0`, `< 0.1` fallback); `C018` (`0x1418f70f0`,
  `evaluate_quest_manager_record`) and `C019` (`0x1418f7180`,
  `evaluate_quest_record`; both F-650); `C027` (`0x14177ac40`/`0x14177ec20`,
  F-676). `C007` (F-657), `C017`
  (`0x1413fd880`/`0x14140c8f0`/`0x141446df0`/`0x1413cfe80`) and `C025`
  (F-648) match GUID. `C013` (`0x1418ac8f0`) destroys
  `FLODTiles \ SpawnedActors` (F-716); `C016` (`0x1416f1be0`/`0x1416f8980`, F-651);
  `C026` (`0x1418f5540`) runs the life restore and cosmetic-armour override filter
  (F-648, F-657).
- `DoesRecorderMatchActor` (`+0x478`, F-904): base
  checks `+0xc0`; `C001` checks GUID if `HasValidOwner` else `+0xc8`;
  `C025` checks `+0xfc >= 1` and `+0xd8/+0xe8`; `C026` checks
  `+0x6c8` and `+0x1c0/+0x1d0`; `C031`/`C034` check `<= 0.1` per axis (`+0x1e0`).
- Per-class record, restore and transfer rules for `C002`..`C006`, `C008`..`C010`,
  `C012`, `C014`, `C020`..`C024`, `C030`, `C032`, `C033`, `C035`, `C036` and `C038`
  (F-657, F-744, F-677, F-646, F-661, F-658, F-648, F-656, F-894, F-736, F-734,
  F-903, F-676) live in `experimental::restore_rules`; each function's doc
  comment carries its server / client addresses.
- C001 restoration follows F-735; C015 clocks/currencies/missions F-653;
  C037 world-boss rows F-903. Recorded level `-1` is unrecorded (F-648).
- `UIcarusGameStateRecorder` pipeline (F-750, F-902):
  - `BeginRecording` (`0x1414961b0`/`0x1414d0900`) rejects
    null or pending-kill components and GameplayTag `0x65`-excluded owners;
    returns `true` if `bIsRecording` (`+0xd1`); while `bIsRestoring` (`+0x128`)
    `AddUnique`s to `DeferredBeginRecordingComponents` (`+0x130`); else adds to
    `RecorderComponents` (`+0x28`), sets `bIsRecording`, binds owner callbacks
    (`0x1418a3d90`) if `HasValidOwner && !bOwnerCallbacksBound` (`+0xd2`), and
    maps `SpawnLocation -> index` in `SpawnLocationToRecorderIndexMap` (`+0xd8`)
    if `HasSavedSpawnLocation` (`+0x420`) and `SpawnLocation != 0`.
  - `EndRecording` (`0x14149fcf0`/`0x1414da140` + `0x1414e6140`)
    removes from `+0x130` and, if recording, `RemoveAtSwap`s from `+0x28`,
    re-indexes the swapped-in tail in `+0xd8`, zeroes `WORD [+0xd1]`
    (`bIsRecording`, `bOwnerCallbacksBound`) and drops its spawn location from
    `+0xd8`; `FlushDeferredRecorder` re-runs `BeginRecording`.
  - `TryRestoreActorRecorder` (`0x1414bf530`/`0x1414f6650`) checks `+0xd8`
    when `RootComponent`
    has non-zero translation and `Mobility != 2`, then scans `+0x28` with
    `+0x478`, calls `ResolveOwnerRecorderState` (`0x1414b6fc0`/`0x1414ef1e0`), and runs
    `+0x468` iff `ManuallyResolved && RequiresStitching()` (`+0x470`).
  - Also matching: `HasValidOwner`, `RecordToProspectSaveState`
    (`0x1414b4680`/`0x1414ecdd0`), `ReadFromProspectSaveState`, `RetainRecorder`
    (`0x1414b86b0`/`0x1414f0300`), `PerformStitchingPass`
    (`0x1414b1d70`/`0x1414eaae0`) and the `N001..N027` slot overrides (`N002`,
    `N003`, `N004`, `N007`, `N010`..`N014`, `N016`, `N020`, `N026`, `N027`).

- Saved row references and FLOD tile matching/respawn follow F-897; packaged
  row meanings remain excluded (F-660).
  Trace points: crop seeds `0x14196e790`, stomach modifiers `0x141890940`,
  FLOD name lookup `0x14189b800` and respawn `0x1418ac0f0`.
- Static-only modifier reload duration uses a distinct callback (F-930): both
  builds store base duration, call authority-gated remaining-time restoration,
  restore cached multiplier, clear any old timer, select setup for finite
  positive remaining time, and call `OnRep` (`u16` tenths match; unordered
  guards differ). Role-`3` expiration wrappers issue reason-`0` `+0x3c0`. On
  primary `UModifierStateComponent`, matched slots, state data, stat scaling,
  scene attachment/ticks, tags/UIDs, lifetime rescaling and ordered activation
  follow F-930 and `experimental::modifier_lifecycle`
  (M-1027/ev_51aa536d68b6, M-1027/ev_c40e7a02c708,
  M-1027/ev_3681a1228c84, M-1027/ev_9785b53a7a36).
  Unresolved/non-`USceneComponent` attachment classes crash.
  Subclass/Blueprint overrides, downstream callee
  effects, owner/row binding and live execution remain unqualified.
  Row declarations (F-1195): `DataRowHandleNew +0xb4` joins FRowHandle;
  `RowName +8` joins FName, declaring `ComparisonIndex +0`/FNameEntryId and
  `Number +4`. Forward FNameEntryId `0x28d8` joins complete `0x310dd`
  (declared size 4); 16 entries weigh 17, including `Value +0`. Number/Value
  declare direct unsigned32 (`0x75`); method identities/empty arglist agree.
- Server 25621760 removal declarations (F-1195): `0x1414b5950` declares
  1059 bytes and selects TPI 2050973 alone (M-1452, M-1454/ev_892924c04bd4).
  Its 28-byte `LF_MFUNCTION` (leaf `0x1009`) declares return 48, class 2050923,
  this 0, convention/attributes 0, count 3, arglist 2050972, this-adjustment 0
  (M-1453, M-1454/ev_b213265d962e, M-1455, M-1456/ev_86dc4a8fa781).
  Class 2050923 is a 76-byte forward `UIcarusFunctionLibrary` `LF_CLASS`
  (leaf `0x1504`, flags 640, size/count/field-list 0); no definition is followed
  (M-1458/ev_e9e2faf6b771).
  The 20-byte `LF_ARGLIST` 2050972 (leaf `0x1201`) declares count 3 matching
  2050973, ordered 7453/1997626/116 (M-1471/ev_b12f5b8b75a1).
  Envelopes: M-1471/ev_ac1d9cdde283. Grammar declares
  7453 `LF_POINTER`: referent 7450, attributes 65548, kind 12/mode 0; no physical
  width/ABI follows. Forward 1997626 names `FModifierStatesRowHandle`,
  flags 640, size/count/field-list/derived/vshape 0 (M-1471/ev_c593a2dcb42c).
  Its complete 1997679 header declares flags 566, size 24, count 30, field-list 1997678,
  derived/vshape 0 (M-1471/ev_85400f1f84f1); no member widths, occupied storage
  or complete semantic definition follows. Retained 7450 `LF_CLASS` names
  forward `AActor`, flags 640, size/count/field-list/derived/vshape 0
  (M-1471/ev_516c488fb7d2). Argument 116 stays numeric/opaque.
  Grammar inherits framing/fixity; no original replay. Prior two complete framing
  passes, 3150352684 physical bytes, full pre/post fixity and independent owned-unit/cgroup
  cleanup (not host-wide): M-1457, M-1458/ev_800189c44abc; receipts M-1472 to M-1476.
  Return/this definitions, storage, aggregate/static ABI, comparator associations,
  native owner/receiver, overrides, coordinates, runtime/import/compatibility stay open.
- `UIcarusFunctionLibrary::RemoveModifierState` matches local selection in
  25621760/25793767 (F-937): filter `+0x8b & 2`, prefer first explicit UID
  match (`-1` disables), else last comparison match; request `+0x3c0(0)`.
  Models take supplied results. F-940's 11-byte server helper copies
  `u64[RCX+8]` to `[RDX]`, returns `RDX`; hidden-return ABI is unproved.
  Entry `C=RCX,D=RDX`: calls sample `D+8`, then `C+8` into distinct stack
  slots before AL equality, not an initial snapshot. Selected call:
  `V=load64[RDI],P=load64[RBP+0x6f],RDX=V+0xb4,RCX=P` gives sources
  `V+0xbc,P+8`; outer RDX=P requires frame/slot preservation.
  Population uses 16-byte source/8-byte append strides and signed32 counts;
  A skips zero, B adds signed-index/pointer tests, not a class/safe-null contract.
  Getters return cached words or pass slot addresses to a common helper.
  Its three STORE8 sites and later LOAD8 require cross-call preservation;
  an intervening call prevents readback identity; its indirect tail is unresolved.
  F-1195 declares types; native element/row storage, requested 24-byte argument
  lowering, ABI, aliases, lifetimes, class/owner/effects and client helper stay
  open. M-1508 binds static checks; M-1509/M-1510 record processes only.

## Candidate

- Float-to-int uses x86 truncation (`i32::MIN`); trig is approximate.

## Open questions

- Data-table rows stay out of scope (F-660).

## Evidence

F-645..F-681, F-716, F-894, F-902..F-904, F-728, F-734..F-736, F-741..F-744, F-746, F-747,
F-750, F-897, F-930, F-937, F-940, F-1195; M-1425 to M-1428, M-1431 to M-1458, M-1471 to M-1476; M-642..M-698, M-755..M-792, M-794..M-807, M-858, M-1024, M-1025,
M-1027, M-1028, M-1035..M-1037, M-1046..M-1048, M-1050, M-1052..M-1054, M-1058, M-1059, M-1061..M-1062, M-1065..M-1068, M-1071..M-1074.
