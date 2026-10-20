# Recorder restoration rules

## Status

Experimental: pure rules in `experimental::restore_rules` (`tests/restore_rules.rs`,
U-045; U-020..U-022, U-032, U-046, U-048, U-050 resolved).
Last reconciled: 2026-10-20.

## Scope

Recorder validate, stitch and restore rules across all 65 `/Script/Icarus.*RecorderComponent`
classes (`C001..C038` and `N001..N027`); save decoding, class lookup, object
construction, script owners and data-table rows are out of scope (F-660).

## Confirmed

- `UIcarusGameStateRecorder` pipeline (`ReadFromProspectSaveState` `0x1414b77e0`,
  `RecordToProspectSaveState` `0x1414b4680`, `PerformStitchingPass` `0x1414b1d70`,
  `RetainRecorder` `0x1414b86b0`, `HasValidOwner` `0x141898750`, F-741, F-750):
  - `has_valid_recorder_owner`: `Outer != nullptr && !Outer->IsPendingKill() && Outer != UIcarusGameStateRecorder::Get(this)`.
  - `ReadFromProspectSaveState` snapshots `InitialRecorders`, runs `HandleRecorderRemoved`
    (`0x14149fcf0`), filters by `Validate` (`+0x408`), calls `ResolveRestoredRecorders`
    (`0x1414bee00`, `evaluate_owner_resolve_step` on `+0xb2`: `FindOnly` `0` -> `+0x450` +
    `0x1414b6fc0` `evaluate_attach_recorder_to_actor`; `RespawnOnly` `1` -> destroys `+0x450`
    before `+0x458`; `FindOrRespawn` `2` -> `+0x450` then `+0x458`; `ManuallyResolved` `3`),
    compacts (`0x14148a160`), resolves `InitialRecorders \ RecorderComponents`
    (`evaluate_unresolved_initial_recorder`: `BeginRecording` `0x1414961b0` if `HasValidOwner()`
    else `RetainRecorder` setting `+0xd2 = 1`, unregistering if `+0x88 & 1`, and renaming `Outer`
    to `this` with flags `1`), and runs `PerformStitchingPass` (`evaluate_stitching_pass_slice`,
    `should_stitch_with_grid_building_actors`: passes `+0x3d0` grid pieces iff
    `bIncludeGridBuildingActors && IsA(UDeployableRecorderComponent) && !PendingKill`).
  - `RecordToProspectSaveState` (`evaluate_record_to_prospect_save_step`) flushes FLOD
    (`0x14185d6c0`), prunes `(!HasValidOwner() && !bIsRetained) || ShouldDropRecorder()`
    (`+0x410`, calling `+0x418`), calls `RecordState` (`0x1418a9510` -> `+0x448`) iff
    `HasValidOwner()`, and writes `StateRecorderBlobs`.
- `recorder_restore_dispatch`, `recorder_ctor_defaults` (`C001..C038`),
  `non_blob_recorder_restore_dispatch`, and `non_blob_recorder_ctor_defaults`
  (`N001..N027`, F-727, F-742, F-746, F-747): all 65 classes inherit `+0xa0 = 0x1418aaf70`
  and match ctor flags and vtable slots (`+0xa0..+0x4c0`) across both builds:
  - `C001..C038`: `+0xb0` (`bSaveModifiers`) `0` on `C003, C007..C010, C020, C021, C025, C026, C029, C030, C033, C034`;
    `+0xb1` (`bSkipDefaultOwnerResolution`) `1` on `C015, C020, C025, C026, C031, C034`;
    `+0xb2`: 13 `FindOnly`, 15 `RespawnOnly`, 4 `FindOrRespawn`, 6 `ManuallyResolved`;
    `+0xd8` (`bRecordModifierStates`) `1` on `C026`; `+0xd9` (`bRecordActorTransform`) `0` on
    `C006, C020, C028, C031, C034`; `+0x470` (`evaluate_requires_stitching`): 17 `AlwaysTrue`,
    `C017` `ParentCharacterUID != -1`, 20 `AlwaysFalse`; 31 `C001` classes inherit
    `+0x488 = 0x14189d380` (`TreePrimitive -> Wood` if `v < 1`) and 7 base keep no-op `+0x488`;
    only `C017` overrides `+0x4a0` (`0x141400150`, `upright_rotation`), `C004` overrides `+0x498`
    (`0x14189ea40`), and `C004` (`0x14189a7d0`) and `C032` (`0x1418ddbf0`,
    `transform_spline_point_positions`) override `+0x4a8`/`+0x4b0`.
  - `N001..N027` (`N002`, `N015`, `N018` base; 24 `C001`; `N012` in backup saves):
    `+0xb0 = 0` on `N001, N004, N007, N009, N012, N017..N019, N021, N022`; `+0xb1 = 1` on `N021`;
    `+0xb2`: 5 `FindOnly` (`N002, N003, N006, N022, N023`), 13 `RespawnOnly`
    (`N001, N004, N007, N009..N014, N016, N017, N019, N024`), 8 `FindOrRespawn`, 1
    `ManuallyResolved` (`N018`); `+0xd8 = 0`; `+0xd9 = 0` on `N020`; `+0x470`: 10 `AlwaysFalse`,
    12 `AlwaysTrue`, 5 `CharacterParentUidPresent` (`N010..N014`); `N011` overrides `+0x4a0`
    (`0x141400150`).
  - `N010..N016` hierarchy (F-750): `N010` (`0x14143d900`/`0x14144c1b0`,
    `evaluate_n010_character_record`/`evaluate_n010_character_restore`) transfers `CharacterUID`
    (`+0x1c0`, `0` if no `PlayerState`), `ParentCharacterUID` (`+0x1c4`) and sets `+0x571 = 1`;
    `N014` (`0x1413cc9b0`/`0x1413e0710`, `evaluate_n014_npc_record`/`evaluate_n014_npc_restore`)
    adds 4 `PlayerState` `int32`s (`+0x1c8..+0x1d4`, restored iff `>= 0`) and row handle (`+0x1d8`);
    `N013` (`0x1413e05d0`, `evaluate_n013_mission_npc_restore`) adds mission row (`+0x1e0` iff
    `!IsNone()`); `N011` (`0x1413fd560`, `evaluate_n011_subcomponent_int_transfer`) transfers
    `+0x2d8`; `N012` (`0x1413fd5e0`) transfers juvenile state before `N014`; `N016` (`0x141405160`,
    `evaluate_n016_stitch_blackboard_ints`) jumps to `N014` and writes 3 blackboard `int32`s
    (`+0x1c8, +0x1cc, +0x1e0`).
- `Validate` (`+0x408`, base `0x140899f70` = `true`; F-652, F-663, F-680, F-742, F-747):
  - `UProspectSubsystem` (`0x141704d70`, `+0x30`): `0x141820ba0` and `0x1417fd560` scan `+0x98`
    for `UserID` (`+0x20`, `Stricmp`) and `ChrSlot` (`+0x30`, `resolve_prospect_player_list`).
  - `C011`/`N022` (`0x1418fa010`, `evaluate_rocket_spawn_record`, `validate_rocket_spawn_record`)
    reset `+0x1c8`/`+0x1d8` if `+0x1e0 < 1` or `list.rejects()`; `N018` (`0x14089a160`,
    `no_recorder_record_valid`) returns `false`.
  - `C025` (`0x1418f9de0`, `evaluate_player_recorder_record`) and `C026` (`0x1418f9e80`,
    `evaluate_player_state_record`) require `v >= 1 && !list.rejects()` (`player_record_valid`);
    `C029` (`0x1418f9f70`, `evaluate_rocket_record`, `rocket_restore`) requires `!list.rejects()`
    and restores iff listed and `v > 0` (F-663).
  - `C028` (`0x1418f9f20`, `evaluate_resource_deposit_record`, `resource_deposit_record_valid`)
    rejects `"Super_Cooled_Ice"`, whereas `N020` (`0x144a71348`) returns `true`; `+0x460`
    (`0x1418f76c0`, `should_grant_deposit_map_icon_stat`) grants stat `0xbe` iff
    `+0x1cc && !GetStat(0xbe)` (F-645, F-742, F-747).
  - `C031` (`+0xb2 = 1`) and `C034` (`+0xb2 = 3`, `0x1418fa0e0`, `evaluate_voxel_record`,
    `voxel_record_valid`) reject `RespawnOnly && bIsVoxelFullyMined` (`+0x1d0`, F-680); `+0x460`
    (`0x1418f7bb0`, `evaluate_voxel_database_restore`) scales if `+0xb2 == 1`, builds `1.1x`
    sphere if `+0x1d0 && !+0x1c8`, and restores voxels/`+0x45c` unless `+0x548 & 0x1000` (F-645).
- Stitching matches GUID + ASCII `Stricmp` (`0x141e48e20`; `C009` `evaluate_deployable_record`,
  `evaluate_deployable_stitch`; `C010` `evaluate_drill_stitch`, `< 0.1` fallback; `C018`
  `evaluate_quest_manager_record`, `evaluate_quest_manager_restore`, `C019`
  `evaluate_quest_record`, `evaluate_quest_restore`, `stitch_quest_actor_records`; `C027`
  `record_prebuilt_structure_actors`, `stitch_prebuilt_structure_actors`, F-645..F-676); `C007`
  (`record_trapped_character_guids`, `stitch_trapped_character_guids`), `C017`
  (`evaluate_mount_character_record`, `evaluate_mount_character_restore`,
  `stitch_character_parent_uid`, `evaluate_npc_recorder_respawn`) and `C025`
  (`stitch_player_recorder_assigned_uids`) match GUID; `C013` (`evaluate_flod_record`,
  `compact_and_diff_flod_tiles`) destroys `FLODTiles \ SpawnedActors` (F-716); `C016`
  (`evaluate_container_manager_record`, `evaluate_container_manager_stitch`, F-651); `C026` runs
  `evaluate_player_state_life_restore` and `filter_restored_cosmetic_armour_overrides` (F-648,
  F-657).
- `DoesRecorderMatchActor` (`+0x478`, F-645, F-742, F-743): base (`0x14188ba30`,
  `does_state_recorder_match_actor`) checks `+0xc0`; `C001` (`0x14188b960`,
  `does_actor_recorder_match_actor`) checks GUID if `HasValidOwner` (`0x141898750`) else `+0xc8`;
  `C025` (`0x1418c9390`, `does_player_recorder_match_actor`) checks `+0xfc >= 1` and `+0xd8/+0xe8`;
  `C026` (`0x1418c9450`, `does_player_state_recorder_match_actor`) checks `+0x6c8` and
  `+0x1c0/+0x1d0`; `C031`/`C034` (`0x1418c9580`, `does_voxel_recorder_match_actor`) check
  `<= 0.1`/axis (`+0x1e0`).
- `C002` (`evaluate_base_level_teleport_record`, `evaluate_base_level_teleport_restore`); `C003`
  (`evaluate_bed_spawn_point_players_transfer`); `C004` (`evaluate_building_grid_record`,
  `should_try_fast_path_prebuilt_grid_restore`); `C005` (`evaluate_cave_air_nav_record`,
  `evaluate_cave_air_nav_restore`, F-677); `C006` (`evaluate_cave_entrance_record`,
  `evaluate_cave_entrance_restore`); `C008` (`evaluate_crop_plot_state_transfer`); `C009`
  (`resolve_deployable_setup_variant_index`, F-646); `C010` (`should_auto_activate_respawned_drill`);
  `C012` (`evaluate_enzyme_geyser_record`, `evaluate_enzyme_geyser_restore`); `C014`
  (`evaluate_flod_tile_record`, `evaluate_flod_tile_restore`); `C020`
  (`evaluate_instanced_level_manager_record`, `evaluate_instanced_level_manager_restore`, F-658);
  `C021` (`evaluate_item_state_record`, `resolve_item_recorder_database_guid`); `C022`
  (`evaluate_orbital_exchange_transfer`); `C023` (`should_restore_actor_transform`, F-744); `C024`
  (`evaluate_player_group_subsystem_record`, `evaluate_player_group_subsystem_restore`); `C030`
  (`evaluate_spawn_blocker_transfer`); `C032` (`evaluate_spline_record`,
  `evaluate_spline_recorder_respawn`, F-658); `C033` (`evaluate_tree_record`,
  `should_drop_respawned_tree_recorder_on_resave`, F-656, F-681, F-736); `C035`
  (`evaluate_weather_controller_record`, `evaluate_weather_controller_restore`, F-734); `C036`
  (`evaluate_weather_forecast_record`, `evaluate_weather_forecast_restore`, F-654, F-665); `C038`
  (`evaluate_world_talent_record`, `evaluate_world_talent_stitch`, F-650..F-676).
- `C001` (`evaluate_actor_recorder_base_record`) runs `evaluate_actor_guid_restore`,
  `evaluate_modifier_state_restore`, `evaluate_actor_subcomponent_restore`, `D01` (`0x1418b8f20`),
  `evaluate_legacy_gun_ammo_upgrade`, overflow and `CPF_SaveGame` vars (`0x1418bcff0`, F-735);
  `C015` (`evaluate_game_mode_record`) runs `restored_elapsed_seconds`,
  `restore_game_mode_exported_currencies`, `restore_game_mode_mission_history` and
  `restored_next_meteor_shower_time` (F-653); `C037` (`evaluate_world_boss_record`,
  `restore_world_boss_row_handles`, F-654); `-1` is unrecorded (`recorded_level`, F-648).

## Candidate

- Float-to-int uses x86 truncation (`i32::MIN`); trig is approximate.

## Open questions

- Data-table rows stay out of scope (F-660).

## Evidence

F-645..F-681, F-716, F-727, F-728, F-734..F-736, F-741..F-744, F-746, F-747,
F-750; M-642..M-698, M-755..M-792, M-794..M-803.
