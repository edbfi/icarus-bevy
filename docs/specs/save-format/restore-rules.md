# Recorder restoration rules

## Status

Experimental: pure rules in `experimental::restore_rules` (`tests/restore_rules.rs`);
the server does not call them and interpreted import stays gated (U-045). Scope:
client 25793767 and server 25621760 recorder code, offsets and 65
`RecorderComponent` `FClassParams` (F-727, F-728, F-734..F-736, F-741..F-744;
U-020..U-022, U-032, U-046, U-048, U-050 resolved). Last reconciled: 2026-10-18.

## Scope

How recorders validate, stitch and restore known field values. Not covered:
save decoding, class lookup, object construction, script-side owner code and
data-table row meanings (F-660).

## Confirmed

- `ReadFromProspectSaveState` (`0x1414b77e0`/`0x1414ec3f0`) runs `Validate`
  (`+0x408`) then `StitchAndResolveRecorders` (`0x1414bee00`), dispatching
  `OwnerResolvePolicy` (`+0xb2`, `evaluate_owner_resolve_step`): `FindOnly` (`0`)
  runs `FindOwner` (`+0x450`) + `AttachRecorderToActor` (`0x1414b6fc0`,
  `evaluate_attach_recorder_to_actor`); `RespawnOnly` (`1`) destroys pre-existing
  `FindOwner` actors (`0x143331760`) before `RespawnOwner` (`+0x458`);
  `FindOrRespawn` (`2`) tries `FindOwner` then `RespawnOwner`; `ManuallyResolved`
  (`3`) keeps the recorder; then `+0x468` (`Stitching`) runs (F-741).
- Across `C001`..`C038`, `recorder_restore_dispatch` encodes vtable slots
  (F-727, F-742): all 38 inherit `UObject::Serialize` (`+0xa0`, `0x1418aaf70`);
  31 `C001` subclasses inherit `+0x488 = 0x14189d380` (`TreePrimitive` -> `Wood`
  if `RecordedVersion < 1`) and 7 direct `UIcarusStateRecorderComponent` classes
  (`C013`, `C014`, `C022`, `C025`, `C035..C037`) inherit no-op `+0x488`; only
  `C017` overrides `+0x4a0` (`0x141400150`, zeroing pitch/roll), and only `C004`
  (`0x141888270`) and `C032` (`0x1418c5cd0`) override `+0x4b0`
  (`ApplyRecorderTransform`).
- Only 7 of 38 classes override `Validate` (`+0x408`, base `0x140899f70` /
  `0x1408c9810` = `true`; F-652, F-663, F-680):
  - `UProspectSubsystem` (`0x141704d70`) embeds `ActiveProspectInfo` (`+0x30`):
    `HasActiveProspect` (`0x141820ba0`) and
    `IsPlayerStillAssociatedWithActiveProspect` (`0x1418231d0` -> `0x1417fd560`)
    scan `AssociatedMembers` (`+0x98`, `0x40` B) for `UserID` (`+0x20`, ASCII
    `Stricmp`) and `ChrSlot` (`+0x30`, `resolve_prospect_player_list`).
  - `C011` (`0x1418fa010`) calls `ResetSpawnRecord` (`0x1418ed540`: `+0x1c8 = ""`,
    `+0x1d8 = -1`) if `Version` (`+0x1e0`) `< 1` or `list.rejects()`, returning
    `true` (`validate_rocket_spawn_record`).
  - `C025` (`0x1418f9de0`, `+0xd8`, `+0xfc`) and `C026` (`0x1418f9e80`, `+0x1c0`,
    `+0x220`) require `Version >= 1 && !list.rejects()` (`player_record_valid`);
    `C029` (`0x1418f9f70`, `+0x1c0`, `+0x1f4`) requires `!list.rejects()`, and
    restores iff an active prospect holds the player and `version > 0`, else
    destroys the rocket (F-663).
  - `C028` (`0x1418f9f20`) rejects `ResourceDTKey` (`+0x1c4`) == `"Super_Cooled_Ice"`
    (`0x144a71800`, `resource_deposit_record_valid`; `UResourceDepositIceRecorderComponent`
    `0x144a71348` has `+0x408 = 0x140899f70`), and `+0x460` (`0x1418f76c0`) adds
    `+1` to stat `0xbe` iff `+0x1cc && GetStat(0xbe) == 0`
    (`should_grant_deposit_map_icon_stat`, F-645, F-742).
  - `C031` (`0x1418c24c0`, `+0xb2 = 1`) and `C034` (`0x1418c2650`, `+0xb2 = 3`,
    `0x1418fa0e0`) reject `RespawnOnly (1) && bIsVoxelFullyMined` (`+0x1d0`,
    F-680); `+0x460` (`0x1418f7bb0`, `evaluate_voxel_database_restore`) sets
    scale if `+0xb2 == 1`, synthesizes a `1.1x` sphere if `+0x1d0 && +0x1c8 == 0`,
    and restores voxels, `+0x590`/`+0x594` and `+0x45c` (`+0x1ec > 0.0`) unless
    `+0x548 & 0x1000` (F-645).
- Stitching matches actors by GUID + ASCII `Stricmp` (`0x141e48e20`, F-650, F-663,
  F-676); `C007` (`0x1418ac670`, `stitch_trapped_character_guids`) and `C025`
  (`0x1418f5160`, `+0xf0..0xf8`, `stitch_player_recorder_assigned_uids`) match
  GUID only; `C026` (`0x1418f7320`/`0x1418f5540`) restores life/health
  (`evaluate_player_state_life_restore`) and valid unique `CosmeticArmourOverrides`
  (`+0x228 -> +0xa80`, `filter_restored_cosmetic_armour_overrides`, F-648, F-657).
- `DoesRecorderMatchActor` (`+0x478`, F-645, F-742, F-743): base (`0x14188ba30`,
  `does_state_recorder_match_actor`) checks `GetName() == +0xc0`; `C001`
  (`0x14188b960`, `does_actor_recorder_match_actor`) checks `IcarusActorGUID` if
  `HasValidOwner` (`0x141898750`) else `+0xc8`; `C025` (`0x1418c9390`,
  `does_player_recorder_match_actor`) checks `+0xfc >= 1` and Controller ID
  (`0x141469c40`) == `+0xd8/+0xe8` (`0x14170a750`); `C026` (`0x1418c9450`,
  `does_player_state_recorder_match_actor`) checks Character (`+0x6c8`) and
  `PlayerState` (`+0x478`/`+0x498`) == `+0x1c0/+0x1d0`; `C031`/`C034` (`0x1418c9580`,
  `does_voxel_recorder_match_actor`) also check `AVoxelResource` location (`<= 0.1`/axis
  at `+0x1e0`).
- `C010` (`0x1418a74f0`) checks `D_ItemsStatic` (`+0x153`) for `ActivateDrill`
  (`0x1416f8240`, `should_auto_activate_respawned_drill`); `C022` saves/loads
  `TileFlags` (`+0xd8`, `SaveMap_BP`/`LoadMap_BP`) and `RadarScans` (`+0x258 <->
  +0x128`); `C038` (`0x14192f140`/`0x14193aaa0`) copies `Talents` (`+0x2c8 <->
  +0x1c0`, F-661, F-676); `C002` reads `GetReplicatedProspectGameTime` (`0x1418d9390`,
  F-657, F-734); `C035` keeps events with `row_duration + start <=
  CurrentBlockEndTime()` (`0x14190a5a0`, F-734).
- `C003` (`0x1418b7710`) copies `SpawnPointPlayers` (`+0x290`); `C004` (`0x1418b7770`)
  tries `0x1413a9690` (`should_try_fast_path_prebuilt_grid_restore`) if `bIsPrebuilt`
  (`+0x128`) else `0x141a7ac10`; `C006` (`0x1418b7b40`) copies `CaveEntranceMask`
  (`+0x1c0`); `C021` (`0x1418a7660`) binds `DatabaseGUID` (`+0x140 > 0` else `-1`,
  `resolve_item_recorder_database_guid`); `C023` (`0x141843be0`) uses `C001`
  vtable (`+0xd9 = 1`, `should_restore_actor_transform`, F-744).
- `C001` (`0x1418b6ed0`/`0x1418ba040`) maps `ModifierLifetime == -1` to `100`,
  restores `D01` (`0x1418b8f20`) via 14 `EDynamicItemProperties` (`0x145e0c090`),
  upgrades `v < 2` ammo (`0x1418a3530`), routes overflow to `ForceAddItems`/`+0x288`,
  and restores `CPF_SaveGame` vars (`0x1418bcff0`, F-735).
- `C033` (`0x1418ed770` -> `0x141717f90`) maps `TreePrimitiveMask` (`+0x1c8`) via
  `TreePrimitiveNames` (`+0x340`, `0x141726f50`) to skip destroyed subtrees
  (`0x1417012e0`); `BP_TreeBase_C` runs `execEndRecording` twice if
  `bIsPhysicsDynamic` (`+0x1e0`, F-656, F-681, F-736).
- Elapsed `< 31` uses positive game time (F-653); survival `-1` is unrecorded
  (F-648); mounts keep yaw; containers keep list order (F-651); `C036`
  (`0x1418e9ff0`/`0x1418f86c0`) saves `NowBlock` (`+0xd8`, `0x141917fc0`) and
  restores `+0x2c0..0x2e8` (`0x14192eff0`, F-654, F-665); `C005` (`0x14141e320`)
  unwraps `"BlueprintGeneratedClass"` via `ClassGeneratedBy` (`+0x208`,
  `resolve_cave_ai_class_name`, F-677).

## Candidate

- Float-to-int uses x86 truncation (`i32::MIN`); trig is approximate.

## Open questions

- Data-table rows stay out of scope (F-660).

## Evidence

F-645, F-648, F-650..F-654, F-656..F-658, F-660, F-661, F-663, F-676, F-677,
F-680, F-681, F-716, F-727, F-728, F-734..F-736, F-741..F-744; M-642, M-653,
M-666, M-682..M-692, M-694, M-695, M-698, M-755, M-756, M-768, M-769, M-774,
M-775, M-781..M-792.
