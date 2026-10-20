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

- `ReadFromProspectSaveState` (`0x1414b77e0`/`0x1414ec3f0`) runs `+0x408` then
  `0x1414bee00` (`evaluate_owner_resolve_step` on `+0xb2`): `FindOnly` (`0`) runs
  `+0x450` + `0x1414b6fc0` (`evaluate_attach_recorder_to_actor`); `RespawnOnly` (`1`)
  destroys `+0x450` actors (`0x143331760`) before `+0x458`; `FindOrRespawn` (`2`)
  tries `+0x450` then `+0x458`; `ManuallyResolved` (`3`) keeps recorder; then `+0x468`
  runs (F-741).
- `recorder_restore_dispatch`, `recorder_ctor_defaults` (`C001..C038`),
  `non_blob_recorder_restore_dispatch`, and `non_blob_recorder_ctor_defaults`
  (`N001..N027`, F-727, F-742, F-746, F-747): all 65 classes inherit
  `+0xa0 = 0x1418aaf70` and match ctor flags and vtable slots (`+0xa0..+0x4c0`)
  across server 25621760 and client 25793767:
  - On `C001..C038`: `+0xb0` (`bSaveModifiers`) base `1`, `0` on
    `C003, C007..C010, C020, C021, C025, C026, C029, C030, C033, C034`; `+0xb1`
    (`bSkipDefaultOwnerResolution`) base `0`, `1` on `C015, C020, C025, C026, C031, C034`;
    `+0xb2`: 13 `FindOnly`, 15 `RespawnOnly`, 4 `FindOrRespawn`, 6 `ManuallyResolved`;
    `+0xd8` (`bRecordModifierStates`) base `0`, `1` on `C026`; `+0xd9`
    (`bRecordActorTransform`) base `1`, `0` on `C006, C020, C028, C031, C034`;
    `+0x470` (`evaluate_requires_stitching`): 17 `AlwaysTrue`, `C017`
    `ParentCharacterUID != -1`, 20 `AlwaysFalse`; 31 `C001` classes inherit
    `+0x488 = 0x14189d380` (`TreePrimitive -> Wood` if `v < 1`) and 7 base (`C013`,
    `C014`, `C022`, `C025`, `C035..C037`) keep no-op `+0x488`; only `C017` overrides
    `+0x4a0` (`0x141400150`, `upright_rotation`), only `C004` overrides `+0x498`
    (`0x14189ea40`/`0x1418a4500` vs `0x14189eae0`), and only `C004`
    (`0x14189a7d0`/`0x141888270` vs `0x14189a760`/`0x141887d20`) and `C032`
    (`0x1418ddbf0`/`0x1418c5cd0`, `transform_spline_point_positions`) override
    `+0x4a8`/`+0x4b0`.
  - On `N001..N027` (`3` non-actor classes `N002`, `N015`, `N018` and `24` `C001`
    subclasses; `N012` `UIcarusJuvenileRecorderComponent` appears in backup saves):
    `+0xb0 = 0` on 10 classes (`N001, N004, N007, N009, N012, N017..N019, N021, N022`);
    `+0xb1 = 1` only on `N021`; `+0xb2`: 5 `FindOnly` (`N002, N003, N006, N022, N023`),
    13 `RespawnOnly` (`N001, N004, N007, N009..N014, N016, N017, N019, N024`), 8
    `FindOrRespawn`, 1 `ManuallyResolved` (`N018`); `+0xd8 = 0` on all 24 actor
    classes; `+0xd9 = 0` only on `N020`; `+0x470`: 10 `AlwaysFalse`, 12 `AlwaysTrue`
    (`N001, N003, N004, N007, N008, N016, N017, N019, N024..N027`), 5
    `CharacterParentUidPresent` (`N010..N014`); only `N011` overrides `+0x4a0` to
    `0x141400150`.
- `Validate` (`+0x408`, base `0x140899f70` = `true`; F-652, F-663, F-680, F-742, F-747):
  - `UProspectSubsystem` (`0x141704d70`, `+0x30`): `0x141820ba0` and
    `0x1418231d0 -> 0x1417fd560` scan `+0x98` (`0x40` B) for `UserID` (`+0x20`,
    `Stricmp`) and `ChrSlot` (`+0x30`, `resolve_prospect_player_list`).
  - `C011` and `N022` (`0x1418e8790`/`0x1418fa010`, `evaluate_rocket_spawn_record`,
    `validate_rocket_spawn_record`) reset `+0x1c8`/`+0x1d8` (`0x1418ed540`) if
    `+0x1e0 < 1` or `list.rejects()`.
  - `N018` (`UNoRecorderComponent`, `0x14089a160`/`0x1408cb520`, `no_recorder_record_valid`)
    returns `false` unconditionally.
  - `C025` (`0x1418e8850`/`0x1418f9de0`, `evaluate_player_recorder_record`) and `C026`
    (`0x1418e8b10`/`0x1418f9e80`, `evaluate_player_state_record`) require
    `v >= 1 && !list.rejects()` (`player_record_valid`); `C029`
    (`0x1418e9280`/`0x1418f9f70`, `evaluate_rocket_record`, `rocket_restore`) requires
    `!list.rejects()` and restores iff listed and `v > 0` (F-663).
  - `C028` (`0x1418e90b0`/`0x1418f9f20`, `evaluate_resource_deposit_record`,
    `resource_deposit_record_valid`) rejects `"Super_Cooled_Ice"` (`0x144a71800`),
    whereas subclass `N020` (`UResourceDepositIceRecorderComponent`, `0x144a71348`)
    overrides `+0x408` back to `0x140899f70` (`true`); `+0x460` (`0x1418f76c0`,
    `should_grant_deposit_map_icon_stat`) grants stat `0xbe` iff
    `+0x1cc && !GetStat(0xbe)` (F-645, F-742, F-747).
  - `C031` (`0x1418c24c0`, `+0xb2 = 1`) and `C034` (`0x1418c2650`, `+0xb2 = 3`,
    `0x1418e96c0`/`0x1418fa0e0`, `evaluate_voxel_record`, `voxel_record_valid`) reject
    `RespawnOnly && bIsVoxelFullyMined` (`+0x1d0`, F-680); `+0x460` (`0x1418f7bb0`,
    `evaluate_voxel_database_restore`) scales if `+0xb2 == 1`, builds `1.1x` sphere if
    `+0x1d0 && !+0x1c8`, and restores voxels/`+0x45c` unless `+0x548 & 0x1000`
    (F-645).
- Stitching matches GUID + ASCII `Stricmp` (`0x141e48e20`; `C009`
  `0x1418a15a0`/`0x1418ac8a0`, `evaluate_deployable_record`,
  `evaluate_deployable_stitch`; `C010` `0x1418acbf0`, `evaluate_drill_stitch`, `< 0.1`
  fallback; `C018` `0x1418e67b0`/`0x1418f70f0`/`0x1418f3d30`,
  `evaluate_quest_manager_record`, `evaluate_quest_manager_restore`, `C019`
  `0x1418e6c40`/`0x1418f7180`/`0x1418f44e0`, `evaluate_quest_record`,
  `evaluate_quest_restore`, `stitch_quest_actor_records`; `C027`
  `0x14177ac40`/`0x14177ec20`, `record_prebuilt_structure_actors`,
  `stitch_prebuilt_structure_actors`, F-645..F-676); `C007`
  (`0x1418a13c0`/`0x1418ac670`, `record_trapped_character_guids`,
  `stitch_trapped_character_guids`), `C017`
  (`0x1413fd880`/`0x14140c8f0`/`0x141446df0`/`0x1413cfe80`,
  `evaluate_mount_character_record`, `evaluate_mount_character_restore`,
  `stitch_character_parent_uid`, `evaluate_npc_recorder_respawn`) and `C025`
  (`0x1418f5160`, `stitch_player_recorder_assigned_uids`) match GUID; `C013`
  (`0x1418a1850`/`0x1418ac8f0`, `evaluate_flod_record`, `compact_and_diff_flod_tiles`)
  destroys `FLODTiles \ SpawnedActors` (F-716); `C016` (`0x1416f1be0`/`0x1416f8980`,
  `evaluate_container_manager_record`, `evaluate_container_manager_stitch`, F-651);
  `C026` (`0x1418f7320`/`0x1418f5540`) runs `evaluate_player_state_life_restore` and
  `filter_restored_cosmetic_armour_overrides` (F-648, F-657).
- `DoesRecorderMatchActor` (`+0x478`, F-645, F-742, F-743): base (`0x14188ba30`,
  `does_state_recorder_match_actor`) checks `+0xc0`; `C001` (`0x14188b960`,
  `does_actor_recorder_match_actor`) checks GUID if `HasValidOwner` (`0x141898750`)
  else `+0xc8`; `C025` (`0x1418c9390`, `does_player_recorder_match_actor`) checks
  `+0xfc >= 1` and `+0xd8/+0xe8`; `C026` (`0x1418c9450`,
  `does_player_state_recorder_match_actor`) checks `+0x6c8` and `+0x1c0/+0x1d0`;
  `C031`/`C034` (`0x1418c9580`, `does_voxel_recorder_match_actor`) check `<= 0.1`/axis
  (`+0x1e0`).
- `C002` (`0x1419d5a20`/`0x1419e27c0`, `evaluate_base_level_teleport_record`,
  `evaluate_base_level_teleport_restore`); `C003` (`0x1418a09f0`/`0x1418b7710`,
  `evaluate_bed_spawn_point_players_transfer`); `C004` (`0x1418a0aa0`/`0x1418b7770`,
  `evaluate_building_grid_record`, `should_try_fast_path_prebuilt_grid_restore`);
  `C005` (`0x1418a0d20`/`0x1418b7860`, `evaluate_cave_air_nav_record`,
  `evaluate_cave_air_nav_restore`, F-677); `C006` (`0x1418a1300`/`0x1418b7b40`,
  `evaluate_cave_entrance_record`, `evaluate_cave_entrance_restore`); `C008`
  (`0x1418a1540`/`0x1418b7bb0`, `evaluate_crop_plot_state_transfer`); `C009`
  (`0x14189d2a0`/`0x1418a72a0`, `resolve_deployable_setup_variant_index`, F-646);
  `C010` (`0x1418a74f0`, `should_auto_activate_respawned_drill`); `C012`
  (`0x1418a1790`/`0x1418b7c10`, `evaluate_enzyme_geyser_record`,
  `evaluate_enzyme_geyser_restore`); `C014` (`0x1418a18f0`/`0x1418b7cf0`,
  `evaluate_flod_tile_record`, `evaluate_flod_tile_restore`); `C020`
  (`0x1419d5ba0`/`0x1419e28e0`, `evaluate_instanced_level_manager_record`,
  `evaluate_instanced_level_manager_restore`, F-658); `C021`
  (`0x1418a3540`/`0x1418a7660`, `evaluate_item_state_record`,
  `resolve_item_recorder_database_guid`); `C022` (`0x1418e83a0`/`0x1418f6f90`,
  `evaluate_orbital_exchange_transfer`); `C023` (`0x141843be0`,
  `should_restore_actor_transform`, F-744); `C024` (`0x1418e85b0`/`0x1418f7070`,
  `evaluate_player_group_subsystem_record`, `evaluate_player_group_subsystem_restore`);
  `C030` (`0x1418e9500`/`0x1418f7930`, `evaluate_spawn_blocker_transfer`); `C032`
  (`0x1418e95f0`/`0x1418ed690`, `evaluate_spline_record`,
  `evaluate_spline_recorder_respawn`, F-658); `C033` (`0x1418e9820`/`0x1418ed770`,
  `evaluate_tree_record`, `should_drop_respawned_tree_recorder_on_resave`, F-656,
  F-681, F-736); `C035` (`0x1418e98b0`/`0x1418f8120`,
  `evaluate_weather_controller_record`, `evaluate_weather_controller_restore`, F-734);
  `C036` (`0x1418e9ff0`/`0x1418f86c0`, `evaluate_weather_forecast_record`,
  `evaluate_weather_forecast_restore`, F-654, F-665); `C038`
  (`0x141930060`/`0x14193aaa0`, `evaluate_world_talent_record`,
  `evaluate_world_talent_stitch`, F-650..F-676).
- `C001` (`0x1418a0170`/`0x1418b6ed0`, `evaluate_actor_recorder_base_record`) runs
  `evaluate_actor_guid_restore`, `evaluate_modifier_state_restore`,
  `evaluate_actor_subcomponent_restore`, `D01` (`0x1418b8f20`),
  `evaluate_legacy_gun_ammo_upgrade` (`0x14189a250`), overflow and `CPF_SaveGame`
  vars (`0x1418bcff0`, F-735); `C015` (`0x1418a1e10`/`0x1418b7f30`,
  `evaluate_game_mode_record`) runs `restored_elapsed_seconds`,
  `restore_game_mode_exported_currencies`, `restore_game_mode_mission_history` and
  `restored_next_meteor_shower_time` (F-653); `C037` (`0x1418ea790`/`0x1418f8610`,
  `evaluate_world_boss_record`, `restore_world_boss_row_handles`, F-654); `-1` is
  unrecorded (`recorded_level`, F-648).
- `UIcarusGameStateRecorder` pipeline and `N010..N016` non-blob slots (F-750):
  `HasValidOwner` (`0x141898750`, `has_valid_recorder_owner`) requires
  `Owner != nullptr && !Owner->IsPendingKillPending() && Owner != DefaultOwner`;
  `RecordToProspectSaveState` (`0x1414b4680`/`0x1414ecdd0`,
  `evaluate_record_to_prospect_save_step`) removes null/pending-kill entries,
  runs `RecordOwnerState` (`+0x448`) when `HasValidOwner`, drops `bDropRecordOnNextSave`
  (`+0xd8`) or invalid (`!Validate()`) components, and appends `FStateRecorderBlob`
  only when `bShouldSaveToProspect` (`+0xd9`) is set; `ReadFromProspectSaveState`
  (`0x1414b77e0`/`0x1414ec3f0`, `evaluate_unresolved_initial_recorder`) calls
  `RetainRecorder` (`0x1414b86b0`/`0x1414f0300`) when `!bSkipDefaultOwnerResolution`
  (`+0xb0 == 0`) or `OwnerResolvePolicy == FindOrRespawn (2)`, else `DestroyComponent`;
  `PerformStitchingPass` (`0x1414b1d70`/`0x1414eaae0`, `evaluate_stitching_pass_slice`,
  `should_stitch_with_grid_building_actors`) iterates `[StartIndex, min(StartIndex + MaxCount, Num))`
  (all when `MaxCount <= 0`) and calls `Stitching` (`+0x468`) with `GridBuildingActors`
  when `bIncludeGridActors && !bSkipGridBuildingStitching` (`+0xb1 == 0`) else
  `WorldActors`; `N010` (`0x14144c1b0`/`0x14148a880`, `evaluate_n010_character_record`,
  `evaluate_n010_character_restore`), `N011` (`0x1413fd560`/`0x14140c580`,
  `evaluate_n011_subcomponent_int_transfer`), `N013` (`0x1413e05d0`/`0x14141e110`,
  `evaluate_n013_mission_npc_restore` returning `Some((name, number))` iff `!FName::IsNone()`),
  `N014` (`0x1413fd680`/`0x14140c6f0`, `evaluate_n014_npc_record`,
  `evaluate_n014_npc_restore`), and `N016` (`0x141405160`/`0x141442d00`,
  `evaluate_n016_stitch_blackboard_ints`) match across both builds.

## Candidate

- Float-to-int uses x86 truncation (`i32::MIN`); trig is approximate.

## Open questions

- Data-table rows stay out of scope (F-660).

## Evidence

F-645..F-681, F-716, F-727, F-728, F-734..F-736, F-741..F-744, F-746, F-747,
F-750; M-642..M-698, M-755..M-792, M-794..M-803.
