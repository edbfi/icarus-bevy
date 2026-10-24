# Recorder restoration rules

## Status

Experimental: pure rules in `experimental::restore_rules` (`tests/restore_rules.rs`,
U-045; U-020..U-022, U-032, U-046, U-048, U-050 resolved).
Last reconciled: 2026-10-24.

## Scope

Recorder validate, stitch and restore rules across all 65 `/Script/Icarus.*RecorderComponent`
classes (`C001..C038`, `N001..N027`); save decoding, class lookup, object
construction, script owners and data-table rows are out of scope (F-660).

## Confirmed

- `ReadFromProspectSaveState` (F-741) runs `+0x408`, then `0x1414bee00`
  (`evaluate_owner_resolve_step` / `evaluate_try_resolve_recorder_step`) on
  `+0xb2`: `FindOnly` (`0`) runs `+0x450` + `0x1414b6fc0`
  (`evaluate_attach_recorder_to_actor` / `evaluate_resolve_owner_recorder_state`);
  `RespawnOnly` (`1`) destroys `+0x450` actors (`0x143331760`) before `+0x458`;
  `FindOrRespawn` (`2`) tries `+0x450` then `+0x458`; `ManuallyResolved` (`3`)
  keeps the recorder; then `+0x468` runs (F-902).
- `recorder_restore_dispatch`, `recorder_ctor_defaults` (`C001..C038`),
  `non_blob_recorder_restore_dispatch` and `non_blob_recorder_ctor_defaults`
  (`N001..N027`): all 65 classes inherit `+0xa0 = 0x1418aaf70` and match ctor
  flags and slots `+0xa0..+0x4c0` across server 25621760 and client 25793767
  (F-902, F-904, F-746, F-747):
  - `C001..C038`: `+0xb0` (`bSaveModifiers`) `1`, but `0` on
    `C003, C007..C010, C020, C021, C025, C026, C029, C030, C033, C034`; `+0xb1`
    (`bSkipDefaultOwnerResolution`) `0`, but `1` on `C015, C020, C025, C026, C031, C034`;
    `+0xb2`: 13 `FindOnly`, 15 `RespawnOnly`, 4 `FindOrRespawn`, 6 `ManuallyResolved`;
    `+0xd8` (`bRecordModifierStates`) `1` only on `C026`; `+0xd9`
    (`bRecordActorTransform`) `1`, but `0` on `C006, C020, C028, C031, C034`;
    `+0x470` (`evaluate_requires_stitching`): 17 `AlwaysTrue`, `C017`
    `ParentCharacterUID != -1`, 20 `AlwaysFalse`. 31 `C001` classes inherit
    `+0x488 = 0x14189d380` (F-716); `C013`, `C014`, `C022`, `C025` and `C035..C037`
    keep the no-op. Only `C017` overrides `+0x4a0` (`upright_rotation`), only
    `C004` `+0x498` (`0x14189ea40`/`0x1418a4500` vs `0x14189eae0`), and only
    `C004` (`0x14189a7d0`/`0x141888270` vs `0x14189a760`/`0x141887d20`) and `C032`
    (`0x1418ddbf0`/`0x1418c5cd0`, `transform_spline_point_positions`)
    `+0x4a8`/`+0x4b0`.
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
    `ChrSlot` (`+0x30`, `resolve_prospect_player_list`).
  - `C011` and `N022` (`0x1418e8790`, `evaluate_rocket_spawn_record`,
    `validate_rocket_spawn_record`) reset `+0x1c8`/`+0x1d8` (`0x1418ed540`) if
    `+0x1e0 < 1` or `list.rejects()`.
  - `N018` (`UNoRecorderComponent`, `0x1408cb520`, `no_recorder_record_valid`)
    is always `false`.
  - `C025` (`0x1418e8850`/`0x1418f9de0`, `evaluate_player_recorder_record`) and `C026`
    (`0x1418e8b10`/`0x1418f9e80`, `evaluate_player_state_record`) require
    `v >= 1 && !list.rejects()` (`player_record_valid`); `C029`
    (`0x1418e9280`/`0x1418f9f70`, `evaluate_rocket_record`, `rocket_restore`) requires
    `!list.rejects()` and restores iff listed and `v > 0`.
  - `C028` (`0x1418e90b0`/`0x1418f9f20`, `evaluate_resource_deposit_record`,
    `resource_deposit_record_valid`) rejects `"Super_Cooled_Ice"` (`0x144a71800`);
    subclass `N020` (`UResourceDepositIceRecorderComponent`,
    `evaluate_n020_validate_override`) overrides `+0x408` back to base `true`; `+0x460` (`0x1418f76c0`,
    `should_grant_deposit_map_icon_stat`) grants stat `0xbe` iff
    `+0x1cc && !GetStat(0xbe)` (F-645, F-902).
  - `C031` and `C034` (F-680; `C034` `0x1418e96c0`, `evaluate_voxel_record`,
    `voxel_record_valid`) reject `RespawnOnly && bIsVoxelFullyMined`; `+0x460`
    (`0x1418f7bb0`, `evaluate_voxel_database_restore`) scales if `+0xb2 == 1`,
    builds a `1.1x` sphere if `+0x1d0 && !+0x1c8`, and restores voxels and `+0x45c`
    unless `+0x548 & 0x1000` (F-645).
- Stitching matches GUID plus ASCII `Stricmp` (F-663): `C009`
  (`evaluate_deployable_record`, `evaluate_deployable_stitch`; F-646); `C010`
  (`0x1418acbf0`, `evaluate_drill_stitch`, `< 0.1` fallback); `C018`
  (`0x1418f70f0`, `evaluate_quest_manager_record`, `evaluate_quest_manager_restore`)
  and `C019` (`0x1418f7180`, `evaluate_quest_record`, `evaluate_quest_restore`,
  `stitch_quest_actor_records`; both F-650); `C027` (`0x14177ac40`/`0x14177ec20`,
  `record_prebuilt_structure_actors`, `stitch_prebuilt_structure_actors`, F-676).
  `C007` (`record_trapped_character_guids`,
  `stitch_trapped_character_guids`, F-657), `C017`
  (`0x1413fd880`/`0x14140c8f0`/`0x141446df0`/`0x1413cfe80`,
  `evaluate_mount_character_record`, `evaluate_mount_character_restore`,
  `stitch_character_parent_uid`, `evaluate_npc_recorder_respawn`) and `C025`
  (`stitch_player_recorder_assigned_uids`, F-648) match GUID. `C013` (`0x1418ac8f0`,
  `evaluate_flod_record`, `compact_and_diff_flod_tiles`) destroys
  `FLODTiles \ SpawnedActors` (F-716); `C016` (`0x1416f1be0`/`0x1416f8980`,
  `evaluate_container_manager_record`, `evaluate_container_manager_stitch`, F-651);
  `C026` (`0x1418f5540`) runs `evaluate_player_state_life_restore` and
  `filter_restored_cosmetic_armour_overrides` (F-648, F-657).
- `DoesRecorderMatchActor` (`+0x478`, F-904): base
  (`does_state_recorder_match_actor`) checks `+0xc0`; `C001`
  (`does_actor_recorder_match_actor`) checks GUID if `HasValidOwner` else `+0xc8`;
  `C025` (`does_player_recorder_match_actor`) checks `+0xfc >= 1` and
  `+0xd8/+0xe8`; `C026` (`does_player_state_recorder_match_actor`) checks
  `+0x6c8` and `+0x1c0/+0x1d0`; `C031`/`C034` (`does_voxel_recorder_match_actor`)
  check `<= 0.1` per axis (`+0x1e0`).
- Per-class transfers: `C002` (`0x1419d5a20`, `evaluate_base_level_teleport_record`,
  `evaluate_base_level_teleport_restore`, F-657); `C003` (`0x1418a09f0`,
  `evaluate_bed_spawn_point_players_transfer`, F-744); `C004` (`0x1418a0aa0`,
  `evaluate_building_grid_record`, `should_try_fast_path_prebuilt_grid_restore`,
  F-744); `C005` (`0x1418a0d20`/`0x1418b7860`, `evaluate_cave_air_nav_record`,
  `evaluate_cave_air_nav_restore`, F-677); `C006` (`0x1418a1300`,
  `evaluate_cave_entrance_record`, `evaluate_cave_entrance_restore`, F-744); `C008`
  (`0x1418a1540`/`0x1418b7bb0`, `evaluate_crop_plot_state_transfer`); `C009`
  (`0x14189d2a0`/`0x1418a72a0`, `resolve_deployable_setup_variant_index`, F-646);
  `C010` (`should_auto_activate_respawned_drill`, F-661); `C012`
  (`evaluate_enzyme_geyser_record`, `evaluate_enzyme_geyser_restore`, F-657);
  `C014` (`0x1418a18f0`/`0x1418b7cf0`, `evaluate_flod_tile_record`,
  `evaluate_flod_tile_restore`); `C020` (`0x1419d5ba0`,
  `evaluate_instanced_level_manager_record`,
  `evaluate_instanced_level_manager_restore`, F-658); `C021` (`0x1418a3540`,
  `evaluate_item_state_record`, `resolve_item_recorder_database_guid`, F-744);
  `C022` (`0x1418e83a0`/`0x1418f6f90`, `evaluate_orbital_exchange_transfer`);
  `C023` (`should_restore_actor_transform`, F-744); `C024`
  (`0x1418e85b0`, `evaluate_player_group_subsystem_record`,
  `evaluate_player_group_subsystem_restore`, F-648); `C030`
  (`0x1418e9500`/`0x1418f7930`, `evaluate_spawn_blocker_transfer`); `C032`
  (`0x1418e95f0`, `evaluate_spline_record`, `evaluate_spline_recorder_respawn`,
  F-658); `C033` (`0x1418e9820`/`0x1418ed770`, `evaluate_tree_record`,
  `should_drop_respawned_tree_recorder_on_resave`, F-656, F-894, F-736); `C035`
  (`0x1418e98b0`/`0x1418f8120`, `evaluate_weather_controller_record`,
  `evaluate_weather_controller_restore`, F-734); `C036`
  (`0x1418e9ff0`/`0x1418f86c0`, `evaluate_weather_forecast_record`,
  `evaluate_weather_forecast_restore`, F-903); `C038` (`0x141930060`,
  `evaluate_world_talent_record`, `evaluate_world_talent_stitch`, F-676).
- C001 GUID/modifier/subcomponent/D01/variable restoration and legacy ammo
  upgrades follow F-735. C015 elapsed time, currencies, mission history and next
  meteor transfer follow F-653; C037 world-boss rows follow F-903.
  Recorded level `-1` is unrecorded (F-648).
- `UIcarusGameStateRecorder` pipeline (F-750, F-902):
  - `BeginRecording` (`0x1414961b0`/`0x1414d0900`, `evaluate_begin_recording`) rejects
    null or pending-kill components and GameplayTag `0x65`-excluded owners;
    returns `true` if `bIsRecording` (`+0xd1`); while `bIsRestoring` (`+0x128`)
    `AddUnique`s to `DeferredBeginRecordingComponents` (`+0x130`); else adds to
    `RecorderComponents` (`+0x28`), sets `bIsRecording`, binds owner callbacks
    (`0x1418a3d90`) if `HasValidOwner && !bOwnerCallbacksBound` (`+0xd2`), and
    maps `SpawnLocation -> index` in `SpawnLocationToRecorderIndexMap` (`+0xd8`)
    if `HasSavedSpawnLocation` (`+0x420`) and `SpawnLocation != 0`.
  - `FlushDeferredRecorder` (`evaluate_flush_deferred_recorder`) moves valid
    deferred entries out of `+0x130` and re-runs `BeginRecording`.
  - `EndRecording` (`0x14149fcf0`/`0x1414da140` + `0x1414e6140`, `evaluate_end_recording`)
    removes from `+0x130` and, if recording, `RemoveAtSwap`s from `+0x28`,
    re-indexes the swapped-in tail in `+0xd8`, zeroes `WORD [+0xd1]`
    (`bIsRecording`, `bOwnerCallbacksBound`) and drops its spawn location from
    `+0xd8`.
  - `TryRestoreActorRecorder` (`0x1414bf530`/`0x1414f6650`,
    `evaluate_try_restore_actor_recorder`) checks `+0xd8` when `RootComponent`
    has non-zero translation and `Mobility != 2`, then scans `+0x28` with
    `+0x478`, calls `ResolveOwnerRecorderState` (`0x1414b6fc0`/`0x1414ef1e0`), and runs
    `+0x468` iff `ManuallyResolved && RequiresStitching()` (`+0x470`).
  - Also matching: `HasValidOwner` (`has_valid_recorder_owner`),
    `RecordToProspectSaveState` (`0x1414b4680`/`0x1414ecdd0`,
    `evaluate_record_to_prospect_save_step`), `ReadFromProspectSaveState`
    (`evaluate_unresolved_initial_recorder`), `RetainRecorder`
    (`0x1414b86b0`/`0x1414f0300`), `PerformStitchingPass` (`0x1414b1d70`/`0x1414eaae0`,
    `evaluate_stitching_pass_slice`, `should_stitch_with_grid_building_actors`),
    and the `N001..N027` slot overrides `evaluate_n002_interface_int_transfer`,
    `evaluate_n003_stitch_linked_actor`, `evaluate_n004_subcomponent_pair_transfer`,
    `evaluate_n007_interface_transfer`, `evaluate_n010_character_record`/`restore`,
    `evaluate_n011_subcomponent_int_transfer`, `evaluate_n012_record`/`restore`,
    `evaluate_n013_mission_npc_restore`, `evaluate_n014_npc_record`/`restore`,
    `evaluate_n016_stitch_blackboard_ints`, `evaluate_n020_validate_override`,
    `evaluate_n026_stitch_transfer` and `evaluate_n027_stitch_transfer`.

- Saved row references and FLOD tile matching/respawn follow F-897; packaged
  row meanings remain excluded (F-660).
- Static-only modifier reload duration uses a distinct callback (F-930). Both builds store
  base duration, call authority-gated remaining-time restoration, restore cached
  multiplier, clear any old timer and select setup for finite positive remaining
  time, then call OnRep. Their unordered scheduling guards differ; floating-point
  runtime state is unobserved. Selected remaining-time/OnRep raw operands match
  after relocation, including u16 tenths. Conversion rounding, mapped callees,
  complete creation/expiration effects and owner/row binding remain unqualified.

## Candidate

- Float-to-int uses x86 truncation (`i32::MIN`); trig is approximate.

## Open questions

- Data-table rows stay out of scope (F-660).

## Evidence

F-645..F-681, F-716, F-894, F-902..F-904, F-728, F-734..F-736, F-741..F-744, F-746, F-747,
F-750, F-897, F-930; M-642..M-698, M-755..M-792, M-794..M-807, M-858.
