# Recorder restoration rules

## Status

Experimental: pure rules in `experimental::restore_rules`, tested in
`tests/restore_rules.rs`; the server does not call them and an interpreted
import stays gated (U-045). Scope: client 25793767 recorder code, offsets and
all 65 `RecorderComponent` `FClassParams` equal in server 25621760 (F-727, F-728,
F-734 to F-736; U-020 to U-022, U-032, U-046, U-048, U-050 resolved).
Last reconciled: 2026-10-18.

## Scope

How recorders validate, stitch and restore known field values. Not covered:
save decoding, class lookup, object construction, script-side owner code and
data-table row meanings (F-660).

## Confirmed

- Across all 38 `StateRecorderBlobs` classes (`C001`..`C038`), `recorder_restore_dispatch`
  encodes the verified vtable table (`F-727`): all 38 inherit `UObject::Serialize`
  (`+0xa0`, `0x1418aaf70`); all 31 `C001` subclasses inherit `+0x488 = 0x14189d380`
  (`TreePrimitive` -> `Wood` when `RecordedVersion < 1`), while the 7 direct
  `UIcarusStateRecorderComponent` classes (`C013`, `C014`, `C022`, `C025`,
  `C035..C037`) inherit no-op `+0x488`; only `C017` overrides `+0x4a0`
  (`0x141400150`, zeroing pitch/roll), and only `C004` (`0x141888270`) and
  `C032` (`0x1418c5cd0`) override `+0x4b0` (`ApplyRecorderTransform`).
- Only 7 of the 38 `StateRecorderBlobs` classes override `Validate` (`vftable +
  0x408`, base `0x140899f70` / `0x1408c9810` returning `true`; F-652, F-663, F-680):
  - `UProspectSubsystem` (`0x141704d70` / `0x141715480`) embeds
    `ActiveProspectInfo` (`FProspectInfo`, `0xa0` B) at `+0x30`, with
    `HasActiveProspect` (`0x141820ba0`) testing `!ProspectID.IsEmpty()` (`+0x38 > 1`)
    and `IsPlayerStillAssociatedWithActiveProspect` (`0x1418231d0` -> `FindByKey`
    `0x1417fd560`) scanning `AssociatedMembers` (`+0x98`, `0x40` B stride) for
    `UserID` (`+0x20`, ASCII `Stricmp`) and `ChrSlot` (`+0x30`, default `0`;
    `resolve_prospect_player_list`).
  - `C011` (`URocketSpawnRecorderComponent::Validate`, `0x1418fa010` /
    `0x1418fa340`) calls `ResetSpawnRecord` (`0x1418ed540`, clearing
    `SpawnRecord.AssignedPlayerID = ""` at `+0x1c8` and `ChrSlot = -1` at `+0x1d8`)
    when `RocketSpawnRecorderVersion` (`+0x1e0`) `< 1` or an active prospect
    lacks `(AssignedPlayerID, ChrSlot)`, then returns `true`
    (`validate_rocket_spawn_record`).
  - `C025` (`UPlayerRecorderComponent`, `0x1418f9de0`, `PlayerCharacterID`
    `+0xd8`, `PlayerRecorderVersion` `+0xfc`) requires `Version >= 1` and
    `!list.rejects()`; `C026` (`UPlayerStateRecorderComponent`, `0x1418f9e80`,
    `+0x1c0`, `+0x220`) keeps `Version < 1` or `!list.rejects()`; `C029`
    (`URocketRecorderComponent`, `0x1418f9f70`, `AssignedPlayerCharacterID`
    `+0x1c0`, `+0x1f4`) requires `!list.rejects()`, and `C029` restore applies
    only when an active prospect holds the player and `version > 0`, destroying
    the rocket otherwise (F-663).
  - `C028` (`UResourceDepositRecorderComponent::Validate`, `0x1418f9f20` /
    `0x1418fa250`) rejects `ResourceDTKey` (`+0x1c4`, default `None`) when empty
    or `"Super_Cooled_Ice"` (`0x144a71800`, `resource_deposit_record_valid`).
  - `C031` (`USpawnedVoxelRecorderComponent`, ctor `0x1418c24c0` setting
    `OwnerResolvePolicy` `+0xb2` to `1` `RespawnOnly`) and `C034`
    (`UVoxelRecorderComponent`, ctor `0x1418c2650` setting `+0xb2` to `3`
    `ManuallyResolved`, `Validate` `0x1418fa0e0`) reject
    `OwnerResolvePolicy == RespawnOnly (1) && VoxelSaveData.bIsVoxelFullyMined`
    (`+0x1d0`, F-680).
- Stitching takes the first spawned actor with the saved Icarus id and a
  matching name (F-650, F-676): equal lengths, ASCII case-insensitive, other
  units exact (F-663). Both `FString::Equals(..., IgnoreCase)` (`0x141e48d20`)
  and `Stricmp` (`0x141e48e20` / `0x141e39a00`) test `(c1 | c2) & 0xffffff80 == 0`,
  fold `< 0x80` via `LowerAscii` (`0x144e8e1a0`), and compare `>= 0x80` directly.
- A voxel record owns the actor within 0.1 per axis, inclusive (F-645).
- `C002` teleport cooldowns read `GetReplicatedProspectGameTime` (`0x1418d9390` =
  `GetTimeSeconds` + `GameState + 0x398`), saving the truncated time left or 0
  (F-657, F-734). `C035` weather controller keeps events with
  `row_duration + start <= CurrentBlockEndTime()` (`0x14190a5a0`) and saves/restores
  `TimeElapsed` against `GetLevelTimeElapsedSec` (`0x1414e1390`, F-734).
- `C001` (`0x1418ba040`) maps saved `ModifierLifetime == -1` to `100`, restores
  `D01` slots (`0x1418b8f20`) via 14 `EDynamicItemProperties` (`0x145e0c090`),
  upgrades `v < 2` ammo (`0x1418a3530`), routes rejected slots to `ForceAddItems`
  (when empty) or `Inventory + 0x288`, and restores `CPF_SaveGame` variables and
  `CPF_RepNotify` callbacks (`0x1418bcff0`, F-735).
- `C033` tree respawn (`0x1418ed770` -> `CreateTreeRuntime` `0x141717f90`) passes
  `TreePrimitiveMask` (`+0x1c8`, union of `+0x330` and `+0x308` at `0x1418e8b40`)
  to `ConstructTreePrimitives` (`0x141716cb0` -> `0x141716f30`), where
  `GetNamedTreePrimitiveMask` (`0x141726f50`) maps indices via `TreePrimitiveNames`
  (`+0x340`) so `0x1417012e0` skips destroyed subtrees, then runs
  `OnConstructedTreePrimitives` (`0x141dc0680`), where `BP_TreeBase_C` calls
  `execEndRecording` twice when `bIsPhysicsDynamic` (`+0x1e0`, F-656, F-681, F-736).
- Saved elapsed level seconds under 31 take the positive saved game time (F-653);
  survival level -1 is unrecorded (F-648); a mount keeps only its yaw; container
  entries keep list positions, empty ones skipped (F-651).
- `C036` (`0x1418e9ff0`) calls `GetCurrentForecastInfo` (`0x141917fc0`) to save
  `NowBlock` (`+0xd8`, `now`'s offset into its first containing block and pattern,
  else `now` and first pattern, zeros if none), and `0x1418f86c0` calls
  `QueueRestoreForecastFromDatabase` (`0x14192eff0`) into `+0x2c0..0x2e8` (F-654, F-665).
- `C005` (`0x14141e320`) unwraps `"BlueprintGeneratedClass"` via
  `ClassGeneratedBy->GetName()` (`+0x208`, `resolve_cave_ai_class_name`),
  replaces the times of the first class equal by `Stricmp`, and fires its script
  event; `C030` saves/restores sign text, color and icon (F-677).

## Candidate

- Float-to-int steps follow x86 truncation (`i32::MIN` when out of range); the
  engine's trigonometric approximations are not reproduced bit for bit.

## Open questions

- Packaged data-table row meanings stay out of scope by policy (F-660).

## Evidence

F-645, F-648, F-650 to F-654, F-656, F-657, F-660, F-663, F-676, F-677, F-680,
F-681, F-716, F-727, F-728, F-734 to F-736; M-642, M-666, M-682, M-683, M-687
to M-692, M-694, M-695, M-698, M-755, M-756, M-768, M-769, M-774, M-775, M-781
to M-784.
