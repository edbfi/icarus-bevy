use icarus_bevy::experimental::restore_rules::{
    ACTOR_RECORDER_TRANSFORM_TOLERANCE, ActorGuidRestoreStep, ActorLink,
    ActorSubcomponentRestorePlan, ApplyTransformKind, AttachRecorderOutcome,
    BaseLevelTeleportRecordPlan, BaseLevelTeleportRestorePlan, ByNameLoadOutcome,
    CaveSpawnerMapEntry, ContainerManagerRecordPlan, ContainerManagerStitchPlan,
    DeployableRecordPlan, DeployableStitchPlan, DrillStitchCandidate, DrillStitchResolution,
    DynamicItemPropertyKind, EnzymeGeyserRecordPlan, EnzymeGeyserRestorePlan, FlodTileRecordPlan,
    FlodTileRestorePlan, InstancedLevelManagerRestorePlan, InventorySlotOverflowDisposition,
    ItemStateRecordPlan, LegacyGunAmmoUpgrade, LobbyPrivacy, NpcRecorderRespawnPlan,
    OrbitalExchangeTransferPlan, OwnerResolvePolicy, OwnerResolveStep,
    PROSPECT_SAVE_STATE_CURRENT_VERSION, PlayerList, PlayerRecorderOwnerState,
    PlayerRecorderRecordPlan, PlayerStateLifeRestore, PlayerStateRecordPlan,
    PrebuiltStructureRecordPlan, ProspectAssociatedMember, ProspectBlobDecodeOutcome,
    QuestManagerRecordPlan, QuestRecordPlan, RESTORED_INDEFINITE_MODIFIER_LIFETIME,
    RecordedWeatherEvent, ResourceDepositRecordPlan, RestoreStateKind, RestoredMissionHistoryEntry,
    RestoredProcessingState, RocketRecordPlan, RocketRestore, RocketSpawnRecordPlan,
    SavedWeatherEventEntry, SetupLoadedProspectOutcome, SpawnBlockerState, SplineRecordPlan,
    SplineRespawnDisposition, StitchedPlayerRecorderLinks, TreeRecordPlan,
    VOXEL_LOCATION_TOLERANCE, VersionUpgradeKind, VoxelDatabaseRestorePlan, VoxelRecordPlan,
    WeatherControllerEventCandidate, WeatherForecastRecordPlan, WorldBossRecordPlan,
    WorldTalentStitchPlan, can_activate_prospect_info, compact_and_diff_flod_tiles,
    container_entries, does_actor_recorder_match_actor, does_player_recorder_match_actor,
    does_player_state_recorder_match_actor, does_state_recorder_match_actor,
    does_voxel_recorder_match_actor, evaluate_actor_guid_restore,
    evaluate_actor_subcomponent_restore, evaluate_attach_recorder_to_actor,
    evaluate_base_level_teleport_record, evaluate_base_level_teleport_restore,
    evaluate_bed_spawn_point_players_transfer, evaluate_building_base_decay_transfer,
    evaluate_building_grid_record, evaluate_by_name_load_attempt, evaluate_cave_air_nav_record,
    evaluate_cave_air_nav_restore, evaluate_cave_entrance_record, evaluate_cave_entrance_restore,
    evaluate_container_manager_record, evaluate_container_manager_stitch,
    evaluate_crop_plot_record, evaluate_crop_plot_restore, evaluate_crop_plot_state_transfer,
    evaluate_deployable_record, evaluate_deployable_stitch, evaluate_drill_stitch,
    evaluate_enzyme_geyser_record, evaluate_enzyme_geyser_restore, evaluate_flod_record,
    evaluate_flod_tile_record, evaluate_flod_tile_restore, evaluate_instanced_level_manager_record,
    evaluate_instanced_level_manager_restore, evaluate_item_state_record,
    evaluate_legacy_gun_ammo_upgrade, evaluate_modifier_state_restore,
    evaluate_npc_recorder_respawn, evaluate_orbital_exchange_transfer, evaluate_owner_resolve_step,
    evaluate_player_group_subsystem_record, evaluate_player_group_subsystem_restore,
    evaluate_player_recorder_record, evaluate_player_state_life_restore,
    evaluate_player_state_record, evaluate_prospect_blob_decode, evaluate_quest_manager_record,
    evaluate_quest_record, evaluate_resource_deposit_record, evaluate_rocket_record,
    evaluate_rocket_spawn_record, evaluate_setup_loaded_prospect_attempt,
    evaluate_spawn_blocker_transfer, evaluate_spline_record, evaluate_spline_recorder_respawn,
    evaluate_tree_record, evaluate_voxel_database_restore, evaluate_voxel_record,
    evaluate_weather_controller_record, evaluate_weather_controller_restore,
    evaluate_weather_forecast_record, evaluate_weather_forecast_restore,
    evaluate_world_boss_record, evaluate_world_talent_record, evaluate_world_talent_stitch,
    filter_restored_cosmetic_armour_overrides, migrate_prospect_save_state, names_match,
    normalize_prospect_name, normalize_restored_modifier_lifetime, player_record_valid,
    prospect_ids_match, record_prebuilt_structure_actors, record_trapped_character_guids,
    record_weather_event_elapsed, recorded_level, recorder_restore_dispatch,
    recorder_transform_is_non_identity, remaining_cooldown, replicated_prospect_game_time,
    resolve_cave_ai_class_name, resolve_deployable_setup_variant_index,
    resolve_inventory_slot_restore, resolve_item_recorder_database_guid,
    resolve_prospect_player_list, resource_deposit_record_valid,
    restore_game_mode_exported_currencies, restore_game_mode_mission_history,
    restore_world_boss_row_handles, restored_elapsed_seconds, restored_next_meteor_shower_time,
    restored_weather_event_start_time, rocket_record_valid, rocket_restore,
    should_auto_activate_respawned_drill, should_drop_respawned_tree_recorder_on_resave,
    should_grant_deposit_map_icon_stat, should_record_weather_event,
    should_restore_actor_transform, should_try_fast_path_prebuilt_grid_restore, stitch,
    stitch_character_parent_uid, stitch_player_recorder_assigned_uids,
    stitch_prebuilt_structure_actors, stitch_quest_actor_records, stitch_trapped_character_guids,
    transform_spline_point_positions, upgrade_actor_recorder_component_names, upright_rotation,
    upsert_recorded_variable, validate_rocket_spawn_record, voxel_location_matches,
    voxel_record_valid,
};

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn link_names_fold_ascii_case_only() {
    assert!(names_match(&units("Foundation_C"), &units("fOUNDATION_c")));
    assert!(names_match(&[], &[]));
    assert!(!names_match(&units("Foundation"), &units("Foundations")));
    assert!(!names_match(&units("Base"), &units("Bass")));
    // Non-ASCII units compare exactly: no Unicode case folding.
    assert!(names_match(
        &units("Gr\u{00f6}\u{00df}e"),
        &units("GR\u{00f6}\u{00df}E")
    ));
    assert!(!names_match(&units("\u{00e9}"), &units("\u{00c9}")));
}

#[test]
fn stitching_needs_both_id_and_name() {
    let name = units("Floor_C");
    let other = units("Wall_C");
    let upper = units("FLOOR_C");
    let saved = ActorLink {
        name: &name,
        icarus_id: 7,
    };
    let spawned = [
        ActorLink {
            name: &other,
            icarus_id: 7,
        },
        ActorLink {
            name: &name,
            icarus_id: 8,
        },
        ActorLink {
            name: &upper,
            icarus_id: 7,
        },
        ActorLink {
            name: &name,
            icarus_id: 7,
        },
    ];
    assert_eq!(stitch(saved, spawned), Some(2));
    assert_eq!(stitch(saved, spawned[..2].iter().copied()), None);
    assert_eq!(stitch(saved, []), None);

    // C018 UIcarusQuestManagerRecorderComponent::Stitching (0x1418f3d30 / 0x1418f4540) and
    // C019 UIcarusQuestRecorderComponent::Stitching (0x1418f44e0 / 0x1418f4cf0):
    // resolves each saved (tag, ActorLink) record against SpawnedActors and drops unmatched records.
    let quest_records = [
        (
            "RadarTag",
            ActorLink {
                name: &name,
                icarus_id: 7,
            },
        ),
        (
            "MissingTag",
            ActorLink {
                name: &other,
                icarus_id: 99,
            },
        ),
        (
            "WallTag",
            ActorLink {
                name: &other,
                icarus_id: 7,
            },
        ),
    ];
    assert_eq!(
        stitch_quest_actor_records(&quest_records, &spawned),
        vec![("RadarTag", 2), ("WallTag", 0)]
    );

    // C018 UIcarusQuestManagerRecorderComponent::RecordOwnerState (0x1418e67b0 / 0x1418e7b40) and
    // C019 UIcarusQuestRecorderComponent::RecordOwnerState (0x1418e6c40 / 0x1418e7fd0):
    // filters actor lists to non-null actors that have a UActorStateRecorderComponent.
    assert_eq!(
        evaluate_quest_manager_record::<&str, &str, &str>(
            false,
            Some(("AIcarusQuest", 10)),
            &[("TagA", Some(("Floor_C", 7)))],
            &[],
            "Mission_Alpha",
            true,
            2,
            1800,
            true,
            "Seed_1",
        ),
        None
    );
    assert_eq!(
        evaluate_quest_manager_record(
            true,
            Some(("AIcarusQuest", 10)),
            &[
                ("TagA", Some(("Floor_C", 7))),
                ("TagUnrecorded", None),
                ("TagB", Some(("Wall_C", 8))),
            ],
            &[("PersistTag", Some(("Radar_C", 99))), ("PersistNone", None)],
            "Mission_Alpha",
            true,
            2,
            1800,
            true,
            "Seed_1",
        ),
        Some(QuestManagerRecordPlan {
            current_mission_record: Some(("AIcarusQuest", 10)),
            relevant_actor_records: vec![("Floor_C", "TagA", 7), ("Wall_C", "TagB", 8)],
            relevant_persistent_actor_records: vec![("Radar_C", "PersistTag", 99)],
            faction_mission_row_name: "Mission_Alpha",
            b_mission_active: true,
            mission_difficulty: 2,
            mission_timer: 1800,
            b_timer_active: true,
            selected_seed_row_name: "Seed_1",
        })
    );
    assert_eq!(
        evaluate_quest_record::<&str, &str, &str, &str>(
            false,
            "Quest_DT_Alpha",
            &[("StepA", Some(("Floor_C", 7)))],
            &[],
            &[],
            &["Progress_1"],
        ),
        None
    );
    assert_eq!(
        evaluate_quest_record(
            true,
            "Quest_DT_Alpha",
            &[("StepA", Some(("Floor_C", 7))), ("StepSkip", None)],
            &[("SecSkip", None), ("SecB", Some(("Wall_C", 8)))],
            &[("FName_Pod", Some(("Pod_C", 55))), ("FName_Skip", None)],
            &["Progress_1", "Progress_2"],
        ),
        Some(QuestRecordPlan {
            quest_dt_key: "Quest_DT_Alpha",
            relevant_actor_records: vec![("Floor_C", "StepA", 7)],
            secondary_actor_records: vec![("Wall_C", "SecB", 8)],
            named_actor_records: vec![("Pod_C", "FName_Pod", 55)],
            quest_progress_records: vec!["Progress_1", "Progress_2"],
        })
    );

    // C027 UPrebuiltStructureRecorderComponent::StitchingOperation (0x14177ec20 / 0x14178c680):
    // resolves RelevantActorRecords (+0x1c8) against SpawnedActors in order and omits unresolved links.
    let prebuilt_records = [
        ActorLink {
            name: &name,
            icarus_id: 7,
        },
        ActorLink {
            name: &other,
            icarus_id: 99,
        },
        ActorLink {
            name: &other,
            icarus_id: 7,
        },
    ];
    assert_eq!(
        stitch_prebuilt_structure_actors(&prebuilt_records, &spawned),
        vec![2, 0]
    );

    // C027 UPrebuiltStructureRecorderComponent::RecordOwnerState (0x14177ac40 / 0x14177fe10):
    // records only structure actors that have a UActorStateRecorderComponent and copies StructureRow.RowName.
    assert_eq!(
        record_prebuilt_structure_actors(
            false,
            "Outpost_Alpha",
            &[Some(("Floor_C", 7)), None, Some(("Wall_C", 8))],
        ),
        None
    );
    assert_eq!(
        record_prebuilt_structure_actors(
            true,
            "Outpost_Alpha",
            &[Some(("Floor_C", 7)), None, Some(("Wall_C", 8))],
        ),
        Some(PrebuiltStructureRecordPlan {
            structure_row_name: "Outpost_Alpha",
            relevant_actor_records: vec![("Floor_C", 7), ("Wall_C", 8)],
        })
    );

    // C009 UDeployableRecorderComponent::Stitching (0x1418ac8a0 / 0x1418b0c80):
    // stitches FoundationActorRecord against ABuildingBase candidates and copies UTameWhitelistComponent when present.
    assert_eq!(
        evaluate_deployable_stitch(false, true, saved, &spawned),
        DeployableStitchPlan::default()
    );
    assert_eq!(
        evaluate_deployable_stitch(true, true, saved, &spawned),
        DeployableStitchPlan {
            foundation_actor_index: Some(2),
            restore_tame_whitelist: true,
        }
    );
    assert_eq!(
        evaluate_deployable_stitch(
            true,
            false,
            ActorLink {
                name: &other,
                icarus_id: 99,
            },
            &spawned
        ),
        DeployableStitchPlan {
            foundation_actor_index: None,
            restore_tame_whitelist: false,
        }
    );

    // C009 UDeployableRecorderComponent::RecordOwnerState (0x1418a15a0 / 0x1418a6d20):
    // records FoundationActorRecord iff FoundationActor has a UActorStateRecorderComponent, and
    // records WhitelistedTames + bAllowAllTames iff UTameWhitelistComponent is present.
    assert_eq!(
        evaluate_deployable_record::<&str, i32>(None, None),
        DeployableRecordPlan {
            foundation_actor_record: None,
            tame_whitelist: None,
        }
    );
    assert_eq!(
        evaluate_deployable_record(Some(("Floor_C", 7)), Some((&[101, 102], true))),
        DeployableRecordPlan {
            foundation_actor_record: Some(("Floor_C", 7)),
            tame_whitelist: Some((vec![101, 102], true)),
        }
    );

    // C010 UDrillRecorderComponent::StitchingOperation (0x1418acbf0 / 0x1418b0ef0):
    // stitches FoundationActorRecord in pass 1, or falls back in pass 2 to the first
    // AResourceDeposit within strict < 0.1 per axis of the drill owner's root location.
    let deposit_name = units("BP_DeepMining_OreDeposit_Iron_C");
    let drill_candidates = [
        DrillStitchCandidate {
            icarus_actor_link: Some(ActorLink {
                name: &other,
                icarus_id: 7,
            }),
            is_valid_resource_deposit: true,
            // Boundary delta == 0.1 fails strict `< 0.1`.
            root_location: Some([10.0 + VOXEL_LOCATION_TOLERANCE, 20.0, 30.0]),
        },
        DrillStitchCandidate {
            icarus_actor_link: Some(ActorLink {
                name: &deposit_name,
                icarus_id: 50,
            }),
            is_valid_resource_deposit: true,
            root_location: Some([10.05, 19.95, 30.0]),
        },
        DrillStitchCandidate {
            icarus_actor_link: Some(ActorLink {
                name: &upper,
                icarus_id: 7,
            }),
            is_valid_resource_deposit: false,
            root_location: None,
        },
    ];
    assert_eq!(
        evaluate_drill_stitch(false, Some([10.0, 20.0, 30.0]), saved, &drill_candidates),
        None
    );
    assert_eq!(
        evaluate_drill_stitch(true, Some([10.0, 20.0, 30.0]), saved, &drill_candidates),
        Some(DrillStitchResolution::StitchedFoundationRecord(2))
    );
    let unresolved_foundation = ActorLink {
        name: &name,
        icarus_id: -1,
    };
    assert_eq!(
        evaluate_drill_stitch(
            true,
            Some([10.0, 20.0, 30.0]),
            unresolved_foundation,
            &drill_candidates
        ),
        Some(DrillStitchResolution::FallbackResourceDeposit(1))
    );
    assert_eq!(
        evaluate_drill_stitch(
            true,
            Some([999.0, 20.0, 30.0]),
            unresolved_foundation,
            &drill_candidates
        ),
        None
    );

    // C007 UCharacterTrapRecorderComponent::Stitching (0x1418ac670 / 0x1418b0a50):
    // matches either legacy TrappedCharacterGUID (+0x290) or TrappedCharacterGUIDs (+0x298)
    // in SpawnedActors order.
    assert_eq!(
        stitch_trapped_character_guids(42, &[100, 200], &[10, 200, 42, 300, 100]),
        vec![200, 42, 100]
    );
    assert_eq!(
        stitch_trapped_character_guids(-1, &[], &[10, 20, 30]),
        Vec::<i32>::new()
    );

    // C007 UCharacterTrapRecorderComponent::RecordOwnerState (0x1418a13c0 / 0x1418a6b40):
    // populates TrappedCharacterGUIDs when GetTrappedCharacters returns >= 1 GUID; falls back
    // to GetTrappedCharacter when the list is empty; sets legacy_trapped_guid = -1 otherwise.
    let mut legacy_guid = 77;
    let mut guid_list = vec![1, 2];
    record_trapped_character_guids(
        true,
        &[Some(100), None, Some(200)],
        Some(999),
        &mut legacy_guid,
        &mut guid_list,
    );
    assert_eq!(legacy_guid, 77);
    assert_eq!(guid_list, vec![100, 200]);

    record_trapped_character_guids(true, &[], Some(555), &mut legacy_guid, &mut guid_list);
    assert_eq!(legacy_guid, 555);
    assert!(guid_list.is_empty());

    record_trapped_character_guids(true, &[None], Some(555), &mut legacy_guid, &mut guid_list);
    assert_eq!(legacy_guid, -1);
    assert!(guid_list.is_empty());

    // C013 UFLODRecorderComponent::Stitching (0x1418ac8f0 / 0x1418b12a0) and
    // RecordOwnerState (0x1418a1850 / 0x1418a6fd0):
    // compacts None entries from FLODTiles in place and returns FLODTiles \ SpawnedActors;
    // RecordOwnerState copies NextInstanceFISMID (+0x250 -> +0xd8) iff AFLODManager is valid.
    let mut flod_tiles = vec![Some(10), None, Some(20), Some(30), None, Some(40)];
    let stale_tiles = compact_and_diff_flod_tiles(&mut flod_tiles, &[20, 40, 99]);
    assert_eq!(flod_tiles, vec![Some(10), Some(20), Some(30), Some(40)]);
    assert_eq!(stale_tiles, vec![10, 30]);
    assert_eq!(evaluate_flod_record(false, 4096), None);
    assert_eq!(evaluate_flod_record(true, 4096), Some(4096));

    // C025 UPlayerRecorderComponent::Stitching (0x1418f5160 / 0x1418f58a0):
    // skips 0 UIDs and resolves each non-zero UID to the first matching GUID in its class bucket.
    assert_eq!(
        stitch_player_recorder_assigned_uids(0, 202, 303, &[0, 101], &[201, 202, 202], &[301, 302],),
        StitchedPlayerRecorderLinks {
            dropship_spawn_index: None,
            dropship_index: Some(1),
            gravestone_index: None,
        }
    );
    assert_eq!(
        stitch_player_recorder_assigned_uids(101, 0, 303, &[100, 101], &[202], &[303, 304]),
        StitchedPlayerRecorderLinks {
            dropship_spawn_index: Some(1),
            dropship_index: None,
            gravestone_index: Some(0),
        }
    );
}

#[test]
fn voxel_owner_tolerance_is_inclusive_per_axis() {
    let saved = [100.0, -50.0, 3.0];
    assert!(voxel_location_matches(saved, saved));
    assert!(voxel_location_matches(
        saved,
        [100.0 + VOXEL_LOCATION_TOLERANCE, -50.0, 3.0]
    ));
    assert!(!voxel_location_matches(saved, [100.0, -50.2, 3.0]));
    assert!(!voxel_location_matches(saved, [100.0, -50.0, f32::NAN]));
    assert!(voxel_location_matches([0.0; 3], [0.05, -0.05, 0.0]));

    let owner_name = units("BP_VoxelResource_Iron_42");
    let owner_name_lower = units("bp_voxelresource_iron_42");
    let other_name = units("BP_VoxelResource_Iron_99");

    // UIcarusStateRecorderComponent::DoesRecorderMatchActor (0x14188ba30 / 0x141894350):
    // compares OwnerRecordName (+0xc0) directly against actor->GetName() via Stricmp.
    assert!(does_state_recorder_match_actor(
        &owner_name,
        &owner_name_lower
    ));
    assert!(!does_state_recorder_match_actor(&owner_name, &other_name));

    // UActorStateRecorderComponent::DoesRecorderMatchActor (0x14188b960 / 0x141894280):
    // when recorder_has_valid_owner is true, matches by IcarusActorGUID; when false, matches by OwnerRecordName.
    assert!(does_actor_recorder_match_actor(
        true,
        1234,
        &owner_name,
        Some(1234),
        &other_name,
    ));
    assert!(!does_actor_recorder_match_actor(
        true,
        1234,
        &owner_name,
        Some(9999),
        &owner_name,
    ));
    assert!(!does_actor_recorder_match_actor(
        true,
        1234,
        &owner_name,
        None,
        &owner_name,
    ));
    assert!(does_actor_recorder_match_actor(
        false,
        1234,
        &owner_name,
        None,
        &owner_name_lower,
    ));
    assert!(!does_actor_recorder_match_actor(
        false,
        1234,
        &owner_name,
        Some(1234),
        &other_name,
    ));

    // UVoxelRecorderComponent::DoesRecorderMatchActor (0x1418c9580 / 0x1418cb460):
    // falls back to AVoxelResource root location within 0.1 per axis when base match returns false.
    assert!(does_voxel_recorder_match_actor(
        false,
        1234,
        &owner_name,
        saved,
        None,
        &other_name,
        true,
        Some([100.05, -50.05, 3.0]),
    ));
    assert!(!does_voxel_recorder_match_actor(
        false,
        1234,
        &owner_name,
        saved,
        None,
        &other_name,
        false,
        Some(saved),
    ));
    // Null RootComponent defaults to [0.0, 0.0, 0.0].
    assert!(does_voxel_recorder_match_actor(
        false,
        1234,
        &owner_name,
        [0.05, -0.05, 0.0],
        None,
        &other_name,
        true,
        None,
    ));

    // C025 UPlayerRecorderComponent::DoesRecorderMatchActor (0x1418c9390 / 0x1418cb270) and
    // C026 UPlayerStateRecorderComponent::DoesRecorderMatchActor (0x1418c9450 / 0x1418cb330).
    let uid = units("76561198000000001");
    let uid_alt = units("AbCdEf01");
    let uid_alt_upper = units("ABCDEF01");
    assert!(does_player_recorder_match_actor(
        1,
        &uid_alt,
        2,
        true,
        &uid_alt_upper,
        2
    ));
    assert!(!does_player_recorder_match_actor(
        0,
        &uid_alt,
        2,
        true,
        &uid_alt_upper,
        2
    ));
    assert!(!does_player_recorder_match_actor(
        1,
        &uid_alt,
        2,
        false,
        &uid_alt_upper,
        2
    ));
    assert!(!does_player_recorder_match_actor(
        1,
        &uid_alt,
        2,
        true,
        &uid_alt_upper,
        3
    ));
    assert!(!does_player_recorder_match_actor(
        1, &uid_alt, 2, true, &uid, 2
    ));

    assert!(does_player_state_recorder_match_actor(
        &uid_alt,
        2,
        true,
        true,
        &uid_alt_upper,
        2
    ));
    assert!(!does_player_state_recorder_match_actor(
        &uid_alt,
        2,
        false,
        true,
        &uid_alt_upper,
        2
    ));
    assert!(!does_player_state_recorder_match_actor(
        &uid_alt,
        2,
        true,
        false,
        &uid_alt_upper,
        2
    ));
    assert!(!does_player_state_recorder_match_actor(
        &uid_alt,
        2,
        true,
        true,
        &uid_alt_upper,
        1
    ));
}

#[test]
fn cooldown_saves_whole_units_left() {
    assert_eq!(remaining_cooldown(0.0, 10.0), 0, "no end set");
    assert_eq!(
        remaining_cooldown(-5.0, -10.0),
        0,
        "a non-positive end is unset"
    );
    assert_eq!(remaining_cooldown(130.9, 100.0), 30, "truncated");
    assert_eq!(remaining_cooldown(100.5, 100.0), 0, "under one unit");
    assert_eq!(remaining_cooldown(90.0, 100.0), 0, "already past");
    assert_eq!(
        remaining_cooldown(f32::MAX, -f32::MAX),
        0,
        "overflow truncates to i32::MIN"
    );
    assert_eq!(remaining_cooldown(f32::NAN, 0.0), 0);
}

#[test]
fn elapsed_seconds_fall_back_to_game_time() {
    assert_eq!(restored_elapsed_seconds(30, 1234.9), 1234);
    assert_eq!(restored_elapsed_seconds(31, 1234.9), 31);
    assert_eq!(
        restored_elapsed_seconds(-4, 0.0),
        -4,
        "game time must be positive"
    );
    assert_eq!(
        restored_elapsed_seconds(5, 1e12),
        i32::MIN,
        "the engine truncation overflows"
    );
    assert_eq!(restored_elapsed_seconds(5000, 20.0), 5000);

    // C015 UIcarusGameModeStateRecorderComponent::UpdateOwnerToDatabaseState (0x1418b7f30 / 0x1418bae00):
    // 1. ExportedCurrencies seeds legacy Exotics/Exotics_Red when recorded_version < 2, then upserts saved_currencies.
    assert_eq!(
        restore_game_mode_exported_currencies(
            1,
            "Exotics",
            120,
            "Exotics_Red",
            35,
            &[("Exotics", 150), ("Biomass", 10)],
        ),
        vec![("Exotics", 150), ("Exotics_Red", 35), ("Biomass", 10)]
    );
    assert_eq!(
        restore_game_mode_exported_currencies(
            2,
            "Exotics",
            120,
            "Exotics_Red",
            35,
            &[("Exotics", 150), ("Biomass", 10)],
        ),
        vec![("Exotics", 150), ("Biomass", 10)]
    );

    // 2. MissionHistory keeps only the last entry when recorded_version < 1, all entries when >= 1,
    // and synthesizes an active world quest entry (end_time = -1) when still empty.
    let m1 = RestoredMissionHistoryEntry {
        quest_key: "Quest_A",
        completed: true,
        end_time: 500,
    };
    let m2 = RestoredMissionHistoryEntry {
        quest_key: "Quest_B",
        completed: false,
        end_time: 900,
    };
    assert_eq!(
        restore_game_mode_mission_history(0, &[m1, m2], Some(("Quest_World", false))),
        vec![m2]
    );
    assert_eq!(
        restore_game_mode_mission_history(1, &[m1, m2], Some(("Quest_World", false))),
        vec![m1, m2]
    );
    assert_eq!(
        restore_game_mode_mission_history(1, &[], Some(("Quest_World", true))),
        vec![RestoredMissionHistoryEntry {
            quest_key: "Quest_World",
            completed: true,
            end_time: -1,
        }]
    );
    assert!(restore_game_mode_mission_history::<&str>(1, &[], None).is_empty());

    // 3. NextMeteorShowerTime uses saved value when non-zero, else LevelTimeElapsedSec + MeteorShowerInterval.
    assert_eq!(restored_next_meteor_shower_time(3600, 1000, 900), 3600);
    assert_eq!(restored_next_meteor_shower_time(0, 1000, 900), 1900);

    // C037 UWorldBossRecorderComponent::RecordOwnerState (0x1418ea790 / 0x1418eae50) and
    // UpdateOwnerToDatabaseState (0x1418f8610 / 0x1418f8ce0):
    // RecordOwnerState copies BossSpawnerRow.RowName and BossStatuses, and derives ActiveWorldBosses;
    // UpdateOwnerToDatabaseState overwrites BossStatuses[i].BossRowHandle from ActiveWorldBosses[i].
    assert_eq!(
        evaluate_world_boss_record(false, "Spawner_Row_A", &[("Boss_Alpha", 100)]),
        None
    );
    assert_eq!(
        evaluate_world_boss_record(
            true,
            "Spawner_Row_A",
            &[("Boss_Alpha", 100), ("Boss_Beta", 250)],
        ),
        Some(WorldBossRecordPlan {
            boss_spawner_row_name: "Spawner_Row_A",
            boss_statuses: vec![("Boss_Alpha", 100), ("Boss_Beta", 250)],
            active_world_bosses: vec!["Boss_Alpha", "Boss_Beta"],
        })
    );
    let mut boss_rows = vec!["Boss_Alpha", "Boss_Beta"];
    assert_eq!(
        restore_world_boss_row_handles(
            &mut boss_rows,
            &["Boss_Override_1", "Boss_Override_2", "Boss_Ignored_3"],
        ),
        2
    );
    assert_eq!(boss_rows, vec!["Boss_Override_1", "Boss_Override_2"]);
}

#[test]
fn unrecorded_levels_are_skipped() {
    assert_eq!(recorded_level(-1), None);
    assert_eq!(recorded_level(0), Some(0));
    assert_eq!(recorded_level(-2), Some(-2));

    // C026 UPlayerStateRecorderComponent::UpdateOwnerToDatabaseState (0x1418f7320 / 0x1418f79f0)
    // and Stitching (0x1418f5540 / 0x1418f5c80).
    assert_eq!(
        evaluate_player_state_life_restore(true, 450, true),
        PlayerStateLifeRestore::Alive {
            set_health: Some(450)
        }
    );
    assert_eq!(
        evaluate_player_state_life_restore(true, -1, false),
        PlayerStateLifeRestore::Alive { set_health: None }
    );
    assert_eq!(
        evaluate_player_state_life_restore(false, 450, true),
        PlayerStateLifeRestore::Dead {
            mark_stat_component_dead: true,
            kill_character_on_stitch: true,
        }
    );
    assert_eq!(
        evaluate_player_state_life_restore(false, 0, false),
        PlayerStateLifeRestore::Dead {
            mark_stat_component_dead: true,
            kill_character_on_stitch: false,
        }
    );

    // C026 CosmeticArmourOverrides restore filters invalid row handles and deduplicates in order.
    let (deduped, should_apply) = filter_restored_cosmetic_armour_overrides(&[
        ("Armour_Head_A", true),
        ("Armour_Chest_Invalid", false),
        ("Armour_Head_A", true),
        ("Armour_Arms_B", true),
    ]);
    assert!(should_apply);
    assert_eq!(deduped, vec!["Armour_Head_A", "Armour_Arms_B"]);

    let (empty_deduped, empty_apply) =
        filter_restored_cosmetic_armour_overrides(&[("Armour_Invalid", false)]);
    assert!(!empty_apply);
    assert!(empty_deduped.is_empty());

    // C026 UPlayerStateRecorderComponent::RecordOwnerState (0x1418e8b10 / 0x1418e8e50):
    // sets Version = 1, defaults null RootComponent to [0,0,0], and applies survival transform
    // override (overwriting Location/Rotation iff non-identity).
    assert_eq!(
        evaluate_player_state_record::<&str, &str, &str>(
            false,
            Some(("76561", 0)),
            None,
            true,
            500,
            -1,
            false,
            None,
            None,
            &[],
            None,
        ),
        None
    );
    assert_eq!(
        evaluate_player_state_record::<&str, &str, &str>(
            true,
            None,
            None,
            true,
            500,
            -1,
            false,
            None,
            None,
            &["Armour_Head_A"],
            Some((
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 1.0],
                [90.0, 90.0, 90.0],
            )),
        ),
        Some(PlayerStateRecordPlan {
            version: 1,
            player_id: None,
            location: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0],
            b_is_alive: true,
            health: 500,
            last_seat_index: -1,
            b_is_last_input_gamepad: false,
            survival_stats: None,
            stomach_contents: None,
            cosmetic_armour_overrides: vec!["Armour_Head_A"],
            override_actor_transform: Some((
                [0.0, 0.0, 0.0, 1.0],
                [0.0, 0.0, 0.0],
                [1.0, 1.0, 1.0],
            )),
        })
    );
    assert_eq!(
        evaluate_player_state_record(
            true,
            Some(("76561198000000001", 1)),
            Some(([10.0, 20.0, 30.0], [1.0, 2.0, 3.0])),
            true,
            480,
            2,
            true,
            Some((900, 800, 700, 3700)),
            Some(&["Meat_Cooked"]),
            &["Armour_Head_A"],
            Some((
                [0.0, 0.0, 0.70710677, 0.70710677],
                [100.0, 200.0, 300.0],
                [1.0, 1.0, 1.0],
                [0.0, 90.0, 0.0],
            )),
        ),
        Some(PlayerStateRecordPlan {
            version: 1,
            player_id: Some(("76561198000000001", 1)),
            location: [100.0, 200.0, 300.0],
            rotation: [0.0, 90.0, 0.0],
            b_is_alive: true,
            health: 480,
            last_seat_index: 2,
            b_is_last_input_gamepad: true,
            survival_stats: Some((900, 800, 700, 3700)),
            stomach_contents: Some(vec!["Meat_Cooked"]),
            cosmetic_armour_overrides: vec!["Armour_Head_A"],
            override_actor_transform: Some((
                [0.0, 0.0, 0.70710677, 0.70710677],
                [100.0, 200.0, 300.0],
                [1.0, 1.0, 1.0],
            )),
        })
    );
}

#[test]
fn mounts_restore_upright() {
    let half = std::f32::consts::FRAC_PI_8; // yaw 45 degrees
    let yaw_only = [0.0, 0.0, half.sin(), half.cos()];
    let close =
        |left: [f32; 4], right: [f32; 4]| left.iter().zip(right).all(|(a, b)| (a - b).abs() < 1e-6);
    assert!(close(upright_rotation(yaw_only), yaw_only));
    assert!(close(
        upright_rotation([0.0, 0.0, 0.0, 1.0]),
        [0.0, 0.0, 0.0, 1.0]
    ));
    // Yaw 45 degrees then pitch 30 degrees: the pitch is dropped.
    let (sp, cp) = (std::f32::consts::PI / 12.0).sin_cos();
    let (sy, cy) = half.sin_cos();
    let pitched = [-sp * sy, sp * cy, cp * sy, cp * cy];
    assert!(close(upright_rotation(pitched), yaw_only));
    let [x, y, z, w] = upright_rotation([0.3, -0.2, 0.5, 0.78]);
    assert_eq!((x, y), (0.0, 0.0));
    assert!((z * z + w * w - 1.0).abs() < 1e-6);

    // C017 UIcarusCharacterRecorderComponent::Stitching (0x141446df0 / 0x141484cd0):
    // scans SpawnedActors in order for the first AIcarusCharacter whose IcarusActorGUID (+0x564)
    // equals ParentCharacterUID (+0x1c4) when ParentCharacterUID != -1.
    let spawned_char_guids = [None, Some(10), Some(42), Some(42)];
    assert_eq!(
        stitch_character_parent_uid(false, true, 42, &spawned_char_guids),
        None
    );
    assert_eq!(
        stitch_character_parent_uid(true, false, 42, &spawned_char_guids),
        None
    );
    assert_eq!(
        stitch_character_parent_uid(true, true, -1, &spawned_char_guids),
        None
    );
    assert_eq!(
        stitch_character_parent_uid(true, true, 99, &spawned_char_guids),
        None
    );
    assert_eq!(
        stitch_character_parent_uid(true, true, 42, &spawned_char_guids),
        Some(2)
    );

    // C017 UIcarusNPCRecorderComponent::RespawnOwner (0x1413cfe80 / 0x14140eab0):
    // requires valid World and AISetupRowName, registers positive IcarusActorGUID (+0x140) on
    // UAIStateRecorderSubsystem::BindActorGuid when present, and spawns with bNoFail = true
    // and SpawnCollisionHandlingOverride = 1 (AlwaysSpawn).
    assert_eq!(
        evaluate_npc_recorder_respawn(false, true, "Mount_Moa", 50, Some(50)),
        None
    );
    assert_eq!(
        evaluate_npc_recorder_respawn(true, false, "None", 50, Some(50)),
        None
    );
    assert_eq!(
        evaluate_npc_recorder_respawn(true, true, "Mount_Moa", 0, Some(50)),
        Some(NpcRecorderRespawnPlan {
            ai_setup_row_name: "Mount_Moa",
            registered_guid_in_subsystem: false,
            spawn_icarus_actor_guid: -1,
            b_no_fail: true,
            collision_handling_override: 1,
        })
    );
    assert_eq!(
        evaluate_npc_recorder_respawn(true, true, "Mount_Moa", 50, None),
        Some(NpcRecorderRespawnPlan {
            ai_setup_row_name: "Mount_Moa",
            registered_guid_in_subsystem: false,
            spawn_icarus_actor_guid: 50,
            b_no_fail: true,
            collision_handling_override: 1,
        })
    );
    assert_eq!(
        evaluate_npc_recorder_respawn(true, true, "Mount_Moa", 50, Some(77)),
        Some(NpcRecorderRespawnPlan {
            ai_setup_row_name: "Mount_Moa",
            registered_guid_in_subsystem: true,
            spawn_icarus_actor_guid: 77,
            b_no_fail: true,
            collision_handling_override: 1,
        })
    );
}

#[test]
fn player_list_validation() {
    let lacking = PlayerList::Active { has_player: false };
    let holding = PlayerList::Active { has_player: true };
    for list in [PlayerList::Absent, PlayerList::Inactive, holding] {
        assert!(player_record_valid(1, list));
        assert!(rocket_record_valid(list));
    }
    assert!(!player_record_valid(1, lacking));
    assert!(!rocket_record_valid(lacking));
    assert!(
        !player_record_valid(0, holding),
        "player records need version 1"
    );
    assert_eq!(rocket_restore(1, holding), RocketRestore::Apply);
    for (version, list) in [
        (1, lacking),
        (1, PlayerList::Absent),
        (1, PlayerList::Inactive),
        (0, holding),
    ] {
        assert_eq!(
            rocket_restore(version, list),
            RocketRestore::Destroy,
            "{version} {list:?}"
        );
    }

    // C025 UPlayerRecorderComponent::RecordOwnerState (0x1418e8850 / 0x1418e8b80):
    // sets SaveDataVersion = 1 unconditionally, preserves DropshipSpawn/Dropship UIDs when null/pending-kill,
    // and clears Gravestone UID to 0 when null/pending-kill.
    assert_eq!(
        evaluate_player_recorder_record::<&str>(
            false,
            Some(("76561", 0)),
            10,
            None,
            20,
            None,
            None
        ),
        PlayerRecorderRecordPlan {
            save_data_version: 1,
            owner_state: None,
        }
    );
    assert_eq!(
        evaluate_player_recorder_record::<&str>(true, None, 10, None, 20, None, None),
        PlayerRecorderRecordPlan {
            save_data_version: 1,
            owner_state: Some(PlayerRecorderOwnerState {
                player_id: None,
                assigned_dropship_spawn_uid: 10,
                assigned_dropship_uid: 20,
                assigned_gravestone_uid: 0,
            }),
        }
    );
    assert_eq!(
        evaluate_player_recorder_record(
            true,
            Some(("76561198000000001", 2)),
            10,
            Some(111),
            20,
            Some(222),
            Some(333),
        ),
        PlayerRecorderRecordPlan {
            save_data_version: 1,
            owner_state: Some(PlayerRecorderOwnerState {
                player_id: Some(("76561198000000001", 2)),
                assigned_dropship_spawn_uid: 111,
                assigned_dropship_uid: 222,
                assigned_gravestone_uid: 333,
            }),
        }
    );

    // C029 URocketRecorderComponent::RecordOwnerState (0x1418e9280 / 0x1418e95c0):
    // sets Version = 1, copies rocket state fields, and updates PlayerID only when AssignedCharacter + PlayerState are valid.
    assert_eq!(
        evaluate_rocket_record::<&str>(
            false,
            2,
            [1.0, 2.0, 3.0],
            [4.0, 5.0, 6.0],
            true,
            false,
            true,
            Some(("76561", 0)),
        ),
        None
    );
    assert_eq!(
        evaluate_rocket_record::<&str>(
            true,
            2,
            [1.0, 2.0, 3.0],
            [4.0, 5.0, 6.0],
            true,
            false,
            true,
            None,
        ),
        Some(RocketRecordPlan {
            version: 1,
            rocket_state: 2,
            spawn_point_location: [1.0, 2.0, 3.0],
            descent_point_location: [4.0, 5.0, 6.0],
            b_has_landed: true,
            b_has_departed: false,
            b_pod_visible: true,
            updated_player_id: None,
        })
    );
}

#[test]
fn prospect_player_list_resolution_rocket_spawn_reset_and_deposit_filter() {
    let prospect_id = units("Olympus_Run");
    let steam_id = units("76561198000000001");
    let steam_id_lower = units("76561198000000001");
    let hex_id = units("AbCdEf01");
    let hex_id_upper = units("ABCDEF01");
    let members = [
        ProspectAssociatedMember {
            user_id: &steam_id,
            chr_slot: 0,
        },
        ProspectAssociatedMember {
            user_id: &hex_id,
            chr_slot: 2,
        },
    ];

    assert_eq!(
        resolve_prospect_player_list(false, &prospect_id, &members, &steam_id, 0),
        PlayerList::Absent
    );
    assert_eq!(
        resolve_prospect_player_list(true, &[], &members, &steam_id, 0),
        PlayerList::Inactive
    );
    assert_eq!(
        resolve_prospect_player_list(true, &prospect_id, &members, &steam_id_lower, 0),
        PlayerList::Active { has_player: true }
    );
    assert_eq!(
        resolve_prospect_player_list(true, &prospect_id, &members, &hex_id_upper, 2),
        PlayerList::Active { has_player: true }
    );
    // Wrong character slot or unknown user ID yields Active { has_player: false }.
    assert_eq!(
        resolve_prospect_player_list(true, &prospect_id, &members, &steam_id, 1),
        PlayerList::Active { has_player: false }
    );
    assert_eq!(
        resolve_prospect_player_list(true, &prospect_id, &members, &units("76561198000000099"), 0),
        PlayerList::Active { has_player: false }
    );

    // C011 URocketSpawnRecorderComponent::RecordOwnerState (0x1418e8790 / 0x1418e97b0) and
    // Validate (0x1418fa010 / 0x1418fa340):
    // RecordOwnerState sets Version = 1 unconditionally and copies AssignedPlayerID iff owner is ADropshipSpawnPoint;
    // Validate resets PlayerID and ChrSlot on version < 1 or Active { has_player: false }, while returning true.
    assert_eq!(
        evaluate_rocket_spawn_record(false, ("76561198000000001", 0)),
        RocketSpawnRecordPlan {
            version: 1,
            assigned_player_id: None,
        }
    );
    assert_eq!(
        evaluate_rocket_spawn_record(true, ("76561198000000001", 2)),
        RocketSpawnRecordPlan {
            version: 1,
            assigned_player_id: Some(("76561198000000001", 2)),
        }
    );
    let mut spawn_player = steam_id.clone();
    let mut spawn_slot = 0;
    assert_eq!(
        validate_rocket_spawn_record(
            1,
            PlayerList::Active { has_player: true },
            &mut spawn_player,
            &mut spawn_slot,
        ),
        (true, false)
    );
    assert_eq!(spawn_player, steam_id);
    assert_eq!(spawn_slot, 0);

    assert_eq!(
        validate_rocket_spawn_record(
            1,
            PlayerList::Active { has_player: false },
            &mut spawn_player,
            &mut spawn_slot,
        ),
        (true, true)
    );
    assert!(spawn_player.is_empty());
    assert_eq!(spawn_slot, -1);

    // Re-running on an already unassigned record (empty id, slot -1) reports was_reset = false.
    assert_eq!(
        validate_rocket_spawn_record(
            0,
            PlayerList::Active { has_player: false },
            &mut spawn_player,
            &mut spawn_slot,
        ),
        (true, false)
    );

    let mut legacy_player = steam_id.clone();
    let mut legacy_slot = 3;
    assert_eq!(
        validate_rocket_spawn_record(
            0,
            PlayerList::Active { has_player: true },
            &mut legacy_player,
            &mut legacy_slot,
        ),
        (true, true)
    );
    assert!(legacy_player.is_empty());
    assert_eq!(legacy_slot, -1);

    // C028 UResourceDepositRecorderComponent::Validate rejects "Super_Cooled_Ice" case-insensitively.
    assert!(!resource_deposit_record_valid(&units("Super_Cooled_Ice")));
    assert!(!resource_deposit_record_valid(&units("super_cooled_ice")));
    assert!(resource_deposit_record_valid(&units("Iron_Deep_Ore")));
    assert!(resource_deposit_record_valid(&units("Exotic_Red")));

    // C028 UResourceDepositRecorderComponent::RecordOwnerState (0x1418e90b0 / 0x1418e93e0) and
    // UpdateOwnerToDatabaseState (0x1418f76c0 / 0x1418f7d90):
    // RecordOwnerState sets bShowMapIcon = (GetStat(0xbe) != 0);
    // UpdateOwnerToDatabaseState grants +1 to stat 0xbe (190) iff bShowMapIcon is true and GetStat(0xbe) == 0.
    assert_eq!(
        evaluate_resource_deposit_record(false, 1200, "Iron_Deep_Ore", 1),
        None
    );
    assert_eq!(
        evaluate_resource_deposit_record(true, 1200, "Iron_Deep_Ore", 0),
        Some(ResourceDepositRecordPlan {
            resource_quantity: 1200,
            deposit_type_row_name: "Iron_Deep_Ore",
            b_show_map_icon: false,
        })
    );
    assert_eq!(
        evaluate_resource_deposit_record(true, 1200, "Iron_Deep_Ore", 2),
        Some(ResourceDepositRecordPlan {
            resource_quantity: 1200,
            deposit_type_row_name: "Iron_Deep_Ore",
            b_show_map_icon: true,
        })
    );
    assert!(should_grant_deposit_map_icon_stat(true, 0));
    assert!(!should_grant_deposit_map_icon_stat(true, 1));
    assert!(!should_grant_deposit_map_icon_stat(false, 0));
}

#[test]
fn container_positions_keep_gaps() {
    let list = [Some('a'), None, Some('c'), None];
    let entries: Vec<_> = container_entries(&list).collect();
    assert_eq!(entries, [(0, &'a'), (2, &'c')]);
    assert_eq!(container_entries::<u8>(&[]).count(), 0);

    // C016 UIcarusContainerManagerRecorderComponent::RecordOwnerState (0x1416f1be0 / 0x141702570)
    // and Stitching (0x1416f8980 / 0x1417099d0).
    assert_eq!(
        evaluate_container_manager_record(false, &list),
        ContainerManagerRecordPlan {
            saved_containers: Vec::new(),
            container_count: 0,
        }
    );
    let recorded_containers = evaluate_container_manager_record(true, &list);
    assert_eq!(
        recorded_containers,
        ContainerManagerRecordPlan {
            saved_containers: vec![(0, 'a'), (2, 'c')],
            container_count: 4,
        }
    );
    assert_eq!(
        evaluate_container_manager_stitch(
            false,
            &recorded_containers.saved_containers,
            recorded_containers.container_count,
            &[Some("ActorInv_1"), None, Some("ActorInv_2")],
        ),
        None
    );
    assert_eq!(
        evaluate_container_manager_stitch(
            true,
            &recorded_containers.saved_containers,
            recorded_containers.container_count,
            &[Some("ActorInv_1"), None, Some("ActorInv_2")],
        ),
        Some(ContainerManagerStitchPlan {
            restored_containers: vec![(0, 'a'), (2, 'c')],
            container_count: 4,
            rebound_actor_containers: vec!["ActorInv_1", "ActorInv_2"],
        })
    );
}

#[test]
fn forecast_position_follows_the_containing_block() {
    use icarus_bevy::experimental::restore_rules::{
        ForecastBlock, ForecastPosition, forecast_position,
    };
    let blocks = [
        ForecastBlock {
            start: 0,
            end: 10,
            pattern_index: 3,
        },
        ForecastBlock {
            start: 10,
            end: 25,
            pattern_index: 7,
        },
    ];
    let at = |start_time_delta, pattern_index, recorded_now| ForecastPosition {
        start_time_delta,
        pattern_index,
        recorded_now,
    };
    assert_eq!(forecast_position(&blocks, 0), at(0, 3, 0));
    assert_eq!(
        forecast_position(&blocks, 10),
        at(0, 7, 10),
        "ends are exclusive"
    );
    assert_eq!(forecast_position(&blocks, 24), at(14, 7, 24));
    assert_eq!(
        forecast_position(&blocks, 40),
        at(40, 3, 40),
        "past every block keeps the first pattern"
    );
    assert_eq!(forecast_position(&blocks, -1), ForecastPosition::default());
    assert_eq!(forecast_position(&[], 5), ForecastPosition::default());

    // C036 UWeatherForecastRecorderComponent::RecordOwnerState (0x1418e9ff0 / 0x1418eadf0 ->
    // GetCurrentForecastInfo 0x141917fc0 / 0x141916980) and UpdateOwnerToDatabaseState
    // (0x1418f86c0 / 0x1418f8c80 -> RestoreForecastInfo 0x14192eff0 / 0x14192be80).
    assert_eq!(
        evaluate_weather_forecast_record(false, "Forecast_A", "Pool_B", &blocks, 10, &[11, 22],),
        None
    );
    assert_eq!(
        evaluate_weather_forecast_record(true, "Forecast_A", "Pool_B", &[], 10, &[11, 22]),
        Some(WeatherForecastRecordPlan {
            row_names: None,
            position: ForecastPosition::default(),
            seeds: Vec::new(),
        })
    );
    assert_eq!(
        evaluate_weather_forecast_record(true, "Forecast_A", "Pool_B", &blocks, -1, &[11, 22]),
        Some(WeatherForecastRecordPlan {
            row_names: None,
            position: ForecastPosition::default(),
            seeds: Vec::new(),
        })
    );
    let recorded_forecast =
        evaluate_weather_forecast_record(true, "Forecast_A", "Pool_B", &blocks, 24, &[11, 22])
            .expect("valid weather forecast record");
    assert_eq!(
        recorded_forecast,
        WeatherForecastRecordPlan {
            row_names: Some(("Forecast_A", "Pool_B")),
            position: at(14, 7, 24),
            seeds: vec![11, 22],
        }
    );
    assert_eq!(
        evaluate_weather_forecast_restore(false, &recorded_forecast),
        None
    );
    assert_eq!(
        evaluate_weather_forecast_restore(true, &recorded_forecast),
        Some(recorded_forecast)
    );
}

#[test]
fn cave_spawn_times_replace_the_first_matching_class() {
    use icarus_bevy::experimental::restore_rules::{CaveSpawnTimes, restore_cave_spawn_times};
    let bp_gen = units("BlueprintGeneratedClass");
    let generated_by = units("BP_CaveWorm_C");
    assert_eq!(
        resolve_cave_ai_class_name(&bp_gen, Some(&generated_by)),
        generated_by.as_slice()
    );
    assert_eq!(resolve_cave_ai_class_name(&bp_gen, None), bp_gen.as_slice());
    assert_eq!(
        resolve_cave_ai_class_name(&generated_by, Some(&bp_gen)),
        generated_by.as_slice()
    );
    let class = |name: &str, times: &[f32]| CaveSpawnTimes {
        class_name: units(name),
        times: times.to_vec(),
    };
    let mut configured = vec![
        class("BP_Wolf", &[1.0]),
        class("bp_wolf", &[2.0]),
        class("BP_Bear", &[3.0]),
    ];
    let saved = [
        class("BP_WOLF", &[10.0, 11.0]),
        class("BP_Bear", &[]),
        class("BP_Unknown", &[5.0]),
    ];
    assert!(restore_cave_spawn_times(&mut configured, &saved));
    assert_eq!(
        configured,
        [
            class("BP_Wolf", &[10.0, 11.0]),
            class("bp_wolf", &[2.0]),
            class("BP_Bear", &[3.0]),
        ],
        "only the first match changes; empty and unmatched records change nothing"
    );
    assert!(
        restore_cave_spawn_times(&mut configured, &[class("BP_Unknown", &[1.0])]),
        "an unmatched class with times still runs the restored event"
    );
    assert!(!restore_cave_spawn_times(
        &mut configured,
        &[class("BP_Wolf", &[])]
    ));
    assert!(!restore_cave_spawn_times(&mut configured, &[]));

    // C005 UCaveAirNavRecorderComponent::UpdateOwnerToDatabaseState (0x1418b7860 / 0x1418ba730):
    // skips clearing owner+0x310 and skips UpdateStateFromDatabase when saved_spawn_data is empty.
    let mut spawner_classes = vec![class("BP_Wolf", &[1.0])];
    assert_eq!(
        evaluate_cave_air_nav_restore(false, &mut spawner_classes, &[class("BP_Wolf", &[9.0])]),
        None
    );
    assert_eq!(
        evaluate_cave_air_nav_restore(true, &mut spawner_classes, &[]),
        None
    );
    assert_eq!(spawner_classes, vec![class("BP_Wolf", &[1.0])]);
    assert_eq!(
        evaluate_cave_air_nav_restore(true, &mut spawner_classes, &[class("BP_Wolf", &[9.0])]),
        Some(true)
    );
    assert_eq!(spawner_classes, vec![class("BP_Wolf", &[9.0])]);

    // C005 UCaveAirNavRecorderComponent::RecordOwnerState (0x1418a0d20 / 0x1418a6480):
    // preserves recorded_spawn_data when owner_is_cave_ai_spawner is false or spawner_map_entries is empty;
    // otherwise clears recorded_spawn_data and appends only valid UClass entries with non-empty times.
    let wolf_uclass = units("Class");
    let mut recorded_spawn_data = vec![class("Prior_Class", &[42.0])];
    assert!(!evaluate_cave_air_nav_record(
        false,
        &[CaveSpawnerMapEntry {
            has_valid_uclass: true,
            uclass_type_name: &bp_gen,
            class_generated_by_name: Some(&generated_by),
            times: &[5.0],
        }],
        &mut recorded_spawn_data,
    ));
    assert!(!evaluate_cave_air_nav_record(
        true,
        &[],
        &mut recorded_spawn_data
    ));
    assert_eq!(recorded_spawn_data, vec![class("Prior_Class", &[42.0])]);
    assert!(evaluate_cave_air_nav_record(
        true,
        &[
            CaveSpawnerMapEntry {
                has_valid_uclass: false,
                uclass_type_name: &bp_gen,
                class_generated_by_name: Some(&generated_by),
                times: &[1.0],
            },
            CaveSpawnerMapEntry {
                has_valid_uclass: true,
                uclass_type_name: &bp_gen,
                class_generated_by_name: Some(&generated_by),
                times: &[],
            },
            CaveSpawnerMapEntry {
                has_valid_uclass: true,
                uclass_type_name: &bp_gen,
                class_generated_by_name: Some(&generated_by),
                times: &[12.5, 25.0],
            },
            CaveSpawnerMapEntry {
                has_valid_uclass: true,
                uclass_type_name: &wolf_uclass,
                class_generated_by_name: None,
                times: &[30.0],
            },
        ],
        &mut recorded_spawn_data,
    ));
    assert_eq!(
        recorded_spawn_data,
        vec![
            class("BP_CaveWorm_C", &[12.5, 25.0]),
            class("Class", &[30.0]),
        ]
    );
}

#[test]
fn saved_owner_policies_map_to_their_enumerators() {
    assert_eq!(
        OwnerResolvePolicy::from_saved(0),
        Some(OwnerResolvePolicy::FindOnly)
    );
    assert_eq!(
        OwnerResolvePolicy::from_saved(1),
        Some(OwnerResolvePolicy::RespawnOnly)
    );
    assert_eq!(
        OwnerResolvePolicy::from_saved(2),
        Some(OwnerResolvePolicy::FindOrRespawn)
    );
    assert_eq!(
        OwnerResolvePolicy::from_saved(3),
        Some(OwnerResolvePolicy::ManuallyResolved)
    );
    assert_eq!(OwnerResolvePolicy::from_saved(4), None);
}

#[test]
fn only_fully_mined_respawned_voxels_fail_validation() {
    // The observed restore dropped the one fully mined respawn-only record
    // and kept partially mined and unmined ones.
    assert!(!voxel_record_valid(OwnerResolvePolicy::RespawnOnly, true));
    assert!(voxel_record_valid(OwnerResolvePolicy::RespawnOnly, false));
    for policy in [
        OwnerResolvePolicy::FindOnly,
        OwnerResolvePolicy::FindOrRespawn,
        OwnerResolvePolicy::ManuallyResolved,
    ] {
        assert!(voxel_record_valid(policy, true));
        assert!(voxel_record_valid(policy, false));
    }

    // UVoxelRecorderComponent::RecordOwnerState (0x1418e96c0 / 0x1418ea6e0):
    // records mined spheres, location (or [0,0,0] when RootComponent is null), voxel counters,
    // ResourceType.RowName iff valid, and overrides OwnerResolvePolicy to RespawnOnly (1) when bCanBreak is set.
    assert_eq!(
        evaluate_voxel_record::<i32, &str>(
            false,
            &[1, 2],
            Some([10.0, 20.0, 30.0]),
            false,
            100,
            60,
            40,
            180.0,
            Some("Iron_Node"),
            true,
        ),
        None
    );
    assert_eq!(
        evaluate_voxel_record(
            true,
            &[1, 2],
            None,
            true,
            100,
            0,
            100,
            180.0,
            None::<&str>,
            false,
        ),
        Some(VoxelRecordPlan {
            mined_spheres: vec![1, 2],
            location: [0.0, 0.0, 0.0],
            b_is_voxel_fully_mined: true,
            max_unmined_voxels: 100,
            current_unmined_voxels: 0,
            num_resources_granted: 100,
            total_resource_count: 180.0,
            resource_type_row_override: None,
            owner_resolve_policy_override: None,
        })
    );
    assert_eq!(
        evaluate_voxel_record(
            true,
            &[1],
            Some([10.0, 20.0, 30.0]),
            false,
            100,
            60,
            40,
            180.0,
            Some("Iron_Node"),
            true,
        ),
        Some(VoxelRecordPlan {
            mined_spheres: vec![1],
            location: [10.0, 20.0, 30.0],
            b_is_voxel_fully_mined: false,
            max_unmined_voxels: 100,
            current_unmined_voxels: 60,
            num_resources_granted: 40,
            total_resource_count: 180.0,
            resource_type_row_override: Some("Iron_Node"),
            owner_resolve_policy_override: Some(OwnerResolvePolicy::RespawnOnly),
        })
    );

    // UVoxelRecorderComponent::UpdateOwnerToDatabaseState (0x1418f7bb0 / 0x1418f8280):
    // 1. Respawned partially mined voxel with explicit spheres and positive TotalResourceCount.
    assert_eq!(
        evaluate_voxel_database_restore(OwnerResolvePolicy::RespawnOnly, false, 3, false, 180.0),
        VoxelDatabaseRestorePlan {
            apply_respawned_world_scale: true,
            synthesize_full_bounds_sphere: false,
            restore_mined_voxels_and_counters: true,
            override_total_resource_count: Some(180.0),
        }
    );
    // 2. ManuallyResolved fully mined voxel with empty MinedSpheres synthesizes 1 full-bounds sphere.
    assert_eq!(
        evaluate_voxel_database_restore(OwnerResolvePolicy::ManuallyResolved, true, 0, false, 0.0),
        VoxelDatabaseRestorePlan {
            apply_respawned_world_scale: false,
            synthesize_full_bounds_sphere: true,
            restore_mined_voxels_and_counters: true,
            override_total_resource_count: None,
        }
    );
    // 3. Disabled actor bit ([AVoxelResource + 0x548] & 0x1000 != 0) or 0 effective spheres skips voxel/counter restore.
    assert_eq!(
        evaluate_voxel_database_restore(
            OwnerResolvePolicy::ManuallyResolved,
            false,
            2,
            true,
            180.0
        ),
        VoxelDatabaseRestorePlan {
            apply_respawned_world_scale: false,
            synthesize_full_bounds_sphere: false,
            restore_mined_voxels_and_counters: false,
            override_total_resource_count: None,
        }
    );
    assert_eq!(
        evaluate_voxel_database_restore(
            OwnerResolvePolicy::ManuallyResolved,
            false,
            0,
            false,
            180.0
        ),
        VoxelDatabaseRestorePlan {
            apply_respawned_world_scale: false,
            synthesize_full_bounds_sphere: false,
            restore_mined_voxels_and_counters: false,
            override_total_resource_count: None,
        }
    );
}

#[test]
fn owner_resolve_step_attach_and_drill_auto_activate() {
    // Already bound owner skips policy dispatch.
    assert_eq!(
        evaluate_owner_resolve_step(
            true,
            OwnerResolvePolicy::FindOnly,
            false,
            false,
            false,
            false
        ),
        OwnerResolveStep::KeptAlreadyBoundOwner
    );

    // FindOnly (0): requires FindOwner + AttachRecorderToActor.
    assert_eq!(
        evaluate_owner_resolve_step(
            false,
            OwnerResolvePolicy::FindOnly,
            false,
            false,
            true,
            true
        ),
        OwnerResolveStep::DroppedFindOnlyOwnerMissing
    );
    assert_eq!(
        evaluate_owner_resolve_step(
            false,
            OwnerResolvePolicy::FindOnly,
            false,
            true,
            false,
            true
        ),
        OwnerResolveStep::DroppedAttachFailed
    );
    assert_eq!(
        evaluate_owner_resolve_step(
            false,
            OwnerResolvePolicy::FindOnly,
            false,
            true,
            true,
            false
        ),
        OwnerResolveStep::AttachedToFoundOwner
    );

    // RespawnOnly (1): destroys pre-existing level owner when !skip_find_and_stitch, then respawns.
    assert_eq!(
        evaluate_owner_resolve_step(
            false,
            OwnerResolvePolicy::RespawnOnly,
            false,
            true,
            false,
            true
        ),
        OwnerResolveStep::RespawnedAndAttached {
            destroyed_preexisting_owner: true
        }
    );
    assert_eq!(
        evaluate_owner_resolve_step(
            false,
            OwnerResolvePolicy::RespawnOnly,
            true,
            true,
            false,
            true
        ),
        OwnerResolveStep::RespawnedAndAttached {
            destroyed_preexisting_owner: false
        }
    );
    assert_eq!(
        evaluate_owner_resolve_step(
            false,
            OwnerResolvePolicy::RespawnOnly,
            false,
            true,
            false,
            false
        ),
        OwnerResolveStep::DroppedRespawnOrAttachFailed {
            destroyed_preexisting_owner: true
        }
    );

    // FindOrRespawn (2): attaches to found owner when !skip_find_and_stitch, else respawns.
    assert_eq!(
        evaluate_owner_resolve_step(
            false,
            OwnerResolvePolicy::FindOrRespawn,
            false,
            true,
            true,
            false
        ),
        OwnerResolveStep::AttachedToFoundOwner
    );
    assert_eq!(
        evaluate_owner_resolve_step(
            false,
            OwnerResolvePolicy::FindOrRespawn,
            false,
            false,
            false,
            true
        ),
        OwnerResolveStep::RespawnedAndAttached {
            destroyed_preexisting_owner: false
        }
    );
    assert_eq!(
        evaluate_owner_resolve_step(
            false,
            OwnerResolvePolicy::FindOrRespawn,
            true,
            true,
            true,
            true
        ),
        OwnerResolveStep::RespawnedAndAttached {
            destroyed_preexisting_owner: false
        }
    );

    // ManuallyResolved (3): keeps directly.
    assert_eq!(
        evaluate_owner_resolve_step(
            false,
            OwnerResolvePolicy::ManuallyResolved,
            false,
            false,
            false,
            false
        ),
        OwnerResolveStep::KeptManuallyResolved
    );

    // AttachRecorderToActor (0x1414b6fc0).
    assert_eq!(
        evaluate_attach_recorder_to_actor(false, false, true, false, false),
        AttachRecorderOutcome::FailedMissingTargetComponent
    );
    assert_eq!(
        evaluate_attach_recorder_to_actor(true, false, false, false, false),
        AttachRecorderOutcome::FailedClassMismatch
    );
    assert_eq!(
        evaluate_attach_recorder_to_actor(true, false, true, false, false),
        AttachRecorderOutcome::RestoredAndUpdatedOwner {
            created_component: false,
            destroyed_temporary_source: true,
        }
    );
    assert_eq!(
        evaluate_attach_recorder_to_actor(false, true, false, false, true),
        AttachRecorderOutcome::RestoredAndUpdatedOwner {
            created_component: true,
            destroyed_temporary_source: false,
        }
    );

    // C009 UDeployableRecorderComponent::ModifyOwnerSpawnParams (0x14189d2a0 / 0x1418a1fb0)
    // and RespawnOwner (0x1418a72a0 / 0x1418abfd0): selects BuildingVariation (5) when in bounds, else 0.
    assert_eq!(resolve_deployable_setup_variant_index(&[(5, 2)], 0), None);
    assert_eq!(
        resolve_deployable_setup_variant_index(&[(6, 100), (5, 2), (5, 1)], 4),
        Some(2)
    );
    assert_eq!(
        resolve_deployable_setup_variant_index(&[(5, 4)], 4),
        Some(0)
    );
    assert_eq!(
        resolve_deployable_setup_variant_index(&[(5, -1)], 4),
        Some(0)
    );
    assert_eq!(
        resolve_deployable_setup_variant_index(&[(6, 100)], 3),
        Some(0)
    );

    // C010 UDrillRecorderComponent::RespawnOwner (0x1418a74f0 / 0x1418abfd0).
    assert!(should_auto_activate_respawned_drill(true, true, true));
    assert!(!should_auto_activate_respawned_drill(true, true, false));
    assert!(!should_auto_activate_respawned_drill(true, false, true));
    assert!(!should_auto_activate_respawned_drill(false, true, true));

    // C004 UBuildingGridRecorderComponent::RecordOwnerState (0x1418a0aa0 / 0x1418a6340) and
    // UpdateOwnerToDatabaseState (0x1418b7770 / 0x1418ba640).
    let mut grid_entries = vec!["Grid_Prior"];
    assert!(!evaluate_building_grid_record(
        false,
        &["Grid_A", "Grid_B"],
        &mut grid_entries
    ));
    assert_eq!(grid_entries, vec!["Grid_Prior"]);
    assert!(evaluate_building_grid_record(
        true,
        &["Grid_A", "Grid_B"],
        &mut grid_entries
    ));
    assert_eq!(grid_entries, vec!["Grid_A", "Grid_B"]);
    assert!(should_try_fast_path_prebuilt_grid_restore(true, true));
    assert!(!should_try_fast_path_prebuilt_grid_restore(true, false));
    assert!(!should_try_fast_path_prebuilt_grid_restore(false, true));

    // C021 UItemStateRecorderComponent::RespawnOwner (0x1418a7660 / 0x1418ac140) and
    // RecordOwnerState (0x1418a3540 / 0x1418a8cc0).
    assert_eq!(resolve_item_recorder_database_guid(55, 77), 77);
    assert_eq!(resolve_item_recorder_database_guid(0, 77), -1);
    assert_eq!(resolve_item_recorder_database_guid(-5, 77), -1);
    assert_eq!(
        evaluate_item_state_record(
            false,
            1,
            "Construct_Params_A",
            "Item_Bench_Anvil",
            &[(5, 2), (6, 1500)],
            &[("Stat_Durability", 100)],
            &["Alteration_A"],
            &[("Living_Slot_1", 450)],
            [1_u32, 2, 3, 4],
            99,
        ),
        None
    );
    assert_eq!(
        evaluate_item_state_record(
            true,
            1,
            "Construct_Params_A",
            "Item_Bench_Anvil",
            &[(5, 2), (6, 1500)],
            &[("Stat_Durability", 100)],
            &["Alteration_A"],
            &[("Living_Slot_1", 450)],
            [1_u32, 2, 3, 4],
            99,
        ),
        Some(ItemStateRecordPlan {
            spawned_context: 1,
            construction_parameters: "Construct_Params_A",
            static_item_data_row_name: "Item_Bench_Anvil",
            dynamic_data: vec![(5, 2), (6, 1500)],
            stats: vec![("Stat_Durability", 100)],
            alterations: vec!["Alteration_A"],
            living_item_slots: vec![(450, "Living_Slot_1")],
            database_guid: [1_u32, 2, 3, 4],
            item_owner_lookup_id: 99,
        })
    );

    // C001/C023 UActorStateRecorderComponent::UpdateOwnerToDatabaseState (0x1418b6ed0 / 0x1418ba040).
    assert!(should_restore_actor_transform(true));
    assert!(!should_restore_actor_transform(false));
    assert_eq!(evaluate_actor_guid_restore(true, true, 50, 10, 50), None);
    assert_eq!(evaluate_actor_guid_restore(false, false, 50, 10, 50), None);
    assert_eq!(evaluate_actor_guid_restore(false, true, 50, 50, 50), None);
    assert_eq!(
        evaluate_actor_guid_restore(false, true, 50, 0, 50),
        Some(ActorGuidRestoreStep {
            unbind_previous_guid: None,
            new_guid: 50,
        })
    );
    assert_eq!(
        evaluate_actor_guid_restore(false, true, 50, 12, 50),
        Some(ActorGuidRestoreStep {
            unbind_previous_guid: Some(12),
            new_guid: 50,
        })
    );
    assert_eq!(
        evaluate_actor_subcomponent_restore(
            true, false, true, true, true, false, 250, true, false, true,
        ),
        ActorSubcomponentRestorePlan {
            restore_durability_component: true,
            generator_active: None,
            processing_state: Some(RestoredProcessingState {
                b_processing_active: true,
                b_is_crafting: false,
                crafting_progress: 250,
                b_resource_1_active: true,
                b_resource_2_active: false,
            }),
            notify_icarus_actor_database_state_updated: true,
        }
    );
    assert_eq!(
        evaluate_actor_subcomponent_restore(
            false, true, false, false, true, true, 100, false, true, false,
        ),
        ActorSubcomponentRestorePlan {
            restore_durability_component: false,
            generator_active: Some(false),
            processing_state: None,
            notify_icarus_actor_database_state_updated: false,
        }
    );

    // C012 UEnzymeGeyserRecorderComponent::RecordOwnerState (0x1418a1790 / 0x1418a6f10) and
    // UpdateOwnerToDatabaseState (0x1418b7c10 / 0x1418baae0):
    // RecordOwnerState copies Completions and CurrentRow.RowName unconditionally;
    // UpdateOwnerToDatabaseState copies Completions unconditionally and CurrentRow iff IsValid.
    assert_eq!(evaluate_enzyme_geyser_record(false, 4, "None"), None);
    assert_eq!(
        evaluate_enzyme_geyser_record(true, 4, "None"),
        Some(EnzymeGeyserRecordPlan {
            completions: 4,
            current_row_name: "None",
        })
    );
    assert_eq!(
        evaluate_enzyme_geyser_restore(false, 4, "Geyser_Reward_A", true),
        None
    );
    assert_eq!(
        evaluate_enzyme_geyser_restore(true, 4, "Geyser_Reward_A", true),
        Some(EnzymeGeyserRestorePlan {
            completions: 4,
            current_row_override: Some("Geyser_Reward_A"),
        })
    );
    assert_eq!(
        evaluate_enzyme_geyser_restore(true, 4, "None", false),
        Some(EnzymeGeyserRestorePlan {
            completions: 4,
            current_row_override: None,
        })
    );

    // C014 UFLODTileRecorderComponent::RecordOwnerState (0x1418a18f0 / 0x1418a7070) and
    // UpdateOwnerToDatabaseState (0x1418b7cf0 / 0x1418babc0):
    // RecordOwnerState copies TileName, TileTransform (or FTransform::Identity when RootComponent is null),
    // Radius, and active component records; UpdateOwnerToDatabaseState restores records whose leading
    // component key resolves via 0x1418970d0(tile, key, 1).
    let flod_records = [(10, "Instance_A"), (20, "Instance_B"), (30, "Instance_C")];
    assert_eq!(
        evaluate_flod_tile_record(false, "Tile_X0_Y0", None, 5000.0, &flod_records),
        None
    );
    assert_eq!(
        evaluate_flod_tile_record(true, "Tile_X0_Y0", None, 5000.0, &flod_records[..2]),
        Some(FlodTileRecordPlan {
            tile_name: "Tile_X0_Y0",
            tile_transform: ([0.0, 0.0, 0.0, 1.0], [0.0, 0.0, 0.0], [1.0, 1.0, 1.0]),
            radius: 5000.0,
            component_records: vec![(10, "Instance_A"), (20, "Instance_B")],
        })
    );
    assert_eq!(
        evaluate_flod_tile_restore(false, "Tile_X0_Y0", 5000.0, &flod_records, &[10, 30]),
        None
    );
    assert_eq!(
        evaluate_flod_tile_restore(true, "Tile_X0_Y0", 5000.0, &flod_records, &[10, 30]),
        Some(FlodTileRestorePlan {
            tile_name: "Tile_X0_Y0",
            radius: 5000.0,
            restored_component_records: vec![(10, "Instance_A"), (30, "Instance_C")],
        })
    );

    // C038 UWorldTalentManagerRecorderComponent::RecordOwnerState (0x141930060 / 0x14192bfd0) and
    // StitchingOperation (0x14193aaa0 / 0x141936a10):
    // RecordOwnerState clears and copies Talents (+0x2c8 -> +0x1c0) iff owner is AWorldTalentManager;
    // StitchingOperation copies Talents (+0x1c0 -> +0x2c8), marks NetComponent (+0x2e8) dirty when active,
    // and notifies authority (0x14193cda0) iff net_role == 3 (ROLE_Authority).
    let mut recorded_talents = vec!["Prior_Talent"];
    assert!(!evaluate_world_talent_record(
        false,
        &["Talent_A", "Talent_B"],
        &mut recorded_talents
    ));
    assert_eq!(recorded_talents, vec!["Prior_Talent"]);
    assert!(evaluate_world_talent_record(
        true,
        &["Talent_A", "Talent_B"],
        &mut recorded_talents
    ));
    assert_eq!(recorded_talents, vec!["Talent_A", "Talent_B"]);
    assert_eq!(
        evaluate_world_talent_stitch(false, &["Talent_A"], true, 3),
        None
    );
    assert_eq!(
        evaluate_world_talent_stitch(true, &["Talent_A", "Talent_B"], false, 3),
        Some(WorldTalentStitchPlan {
            restored_talents: vec!["Talent_A", "Talent_B"],
            mark_net_component_dirty: false,
            notify_authority_talents_updated: false,
        })
    );
    assert_eq!(
        evaluate_world_talent_stitch(true, &["Talent_A"], true, 1),
        Some(WorldTalentStitchPlan {
            restored_talents: vec!["Talent_A"],
            mark_net_component_dirty: true,
            notify_authority_talents_updated: false,
        })
    );
    assert_eq!(
        evaluate_world_talent_stitch(true, &["Talent_A"], true, 3),
        Some(WorldTalentStitchPlan {
            restored_talents: vec!["Talent_A"],
            mark_net_component_dirty: true,
            notify_authority_talents_updated: true,
        })
    );

    // C020 UInstancedLevelManagerRecorderComponent::RecordOwnerState (0x1419d5ba0 / 0x1419ccef0) and
    // UpdateOwnerToDatabaseState (0x1419e28e0 / 0x1419d9b90):
    // gates base UActorStateRecorderComponent::RecordOwnerState and UpdateOwnerToDatabaseState on a valid AInstancedLevelManager.
    assert_eq!(
        evaluate_instanced_level_manager_record(false, "Levels_A", &[1, 2], &[3]),
        None
    );
    assert_eq!(
        evaluate_instanced_level_manager_record(true, "Levels_A", &[1, 2], &[3]),
        Some(InstancedLevelManagerRestorePlan {
            run_base_actor_state_restore: true,
            instanced_levels: "Levels_A",
            spawned_recorder_ids: vec![1, 2],
            destroyed_recorder_ids: vec![3],
        })
    );
    assert_eq!(
        evaluate_instanced_level_manager_restore(false, "Levels_A", &[1, 2], &[3]),
        None
    );
    assert_eq!(
        evaluate_instanced_level_manager_restore(true, "Levels_A", &[1, 2], &[3]),
        Some(InstancedLevelManagerRestorePlan {
            run_base_actor_state_restore: true,
            instanced_levels: "Levels_A",
            spawned_recorder_ids: vec![1, 2],
            destroyed_recorder_ids: vec![3],
        })
    );

    // C024 UPlayerGroupSubsystemRecorderComponent::RecordOwnerState (0x1418e85b0 / 0x1418e88e0)
    // and UpdateOwnerToDatabaseState (0x1418f7070 / 0x1418f7740):
    // RecordOwnerState unconditionally clears recorder->PlayerGroupData (+0x1c0) before checking owner.
    let mut group_data = vec!["Group_Prior"];
    assert!(!evaluate_player_group_subsystem_record(
        false,
        &["Group_A", "Group_B"],
        &mut group_data
    ));
    assert!(group_data.is_empty());
    assert!(evaluate_player_group_subsystem_record(
        true,
        &["Group_A", "Group_B"],
        &mut group_data
    ));
    assert_eq!(group_data, vec!["Group_A", "Group_B"]);
    assert_eq!(
        evaluate_player_group_subsystem_restore(false, &group_data),
        None
    );
    assert_eq!(
        evaluate_player_group_subsystem_restore(true, &group_data),
        Some(vec!["Group_A", "Group_B"])
    );

    // C006 UCaveEntranceRecorderComponent::RecordOwnerState (0x1418a1300 / 0x1418a6a80)
    // and UpdateOwnerToDatabaseState (0x1418b7b40 / 0x1418baa10) (plus legacy aliases).
    let mut cave_spheres = vec!["Sphere_A"];
    assert!(!evaluate_cave_entrance_record(
        false,
        Some(&["Sphere_B"]),
        &mut cave_spheres
    ));
    assert!(!evaluate_cave_entrance_record(
        true,
        None,
        &mut cave_spheres
    ));
    assert_eq!(cave_spheres, vec!["Sphere_A"]);
    assert!(evaluate_cave_entrance_record(
        true,
        Some(&["Sphere_B", "Sphere_C"]),
        &mut cave_spheres
    ));
    assert_eq!(cave_spheres, vec!["Sphere_B", "Sphere_C"]);
    assert_eq!(evaluate_cave_entrance_restore(false, &cave_spheres), None);
    assert_eq!(
        evaluate_cave_entrance_restore(true, &cave_spheres),
        Some(vec!["Sphere_B", "Sphere_C"])
    );
    assert!(evaluate_crop_plot_record(
        true,
        Some(&["Sphere_D"]),
        &mut cave_spheres
    ));
    assert_eq!(
        evaluate_crop_plot_restore(true, &cave_spheres),
        Some(vec!["Sphere_D"])
    );

    // C003 UBedRecorderComponent::RecordOwnerState (0x1418a09f0 / 0x1418a6290)
    // and UpdateOwnerToDatabaseState (0x1418b7710 / 0x1418ba5e0).
    assert_eq!(
        evaluate_bed_spawn_point_players_transfer(false, &["76561198000000001"]),
        None
    );
    assert_eq!(
        evaluate_bed_spawn_point_players_transfer(
            true,
            &["76561198000000001", "76561198000000002"]
        ),
        Some(vec!["76561198000000001", "76561198000000002"])
    );

    // C008 UCropPlotRecorderComponent::RecordOwnerState (0x1418a1540 / 0x1418a6cc0)
    // and UpdateOwnerToDatabaseState (0x1418b7bb0 / 0x1418baa80) (plus legacy alias).
    assert_eq!(evaluate_crop_plot_state_transfer(false, 420), None);
    assert_eq!(evaluate_crop_plot_state_transfer(true, 420), Some(420));
    assert_eq!(evaluate_building_base_decay_transfer(false, 420), None);
    assert_eq!(evaluate_building_base_decay_transfer(true, 420), Some(420));

    // C022 UOrbitalExchangeInterfaceRecorderComponent::RecordOwnerState (0x1418e83a0 / 0x1418e86d0)
    // and UpdateOwnerToDatabaseState (0x1418f6f90 / 0x1418f7660).
    assert_eq!(
        evaluate_orbital_exchange_transfer(false, "Exchange_Inv", &["Pod_Item_1"]),
        None
    );
    assert_eq!(
        evaluate_orbital_exchange_transfer(true, "Exchange_Inv", &["Pod_Item_1", "Pod_Item_2"]),
        Some(OrbitalExchangeTransferPlan {
            exchange_inventories: "Exchange_Inv",
            delivered_items: vec!["Pod_Item_1", "Pod_Item_2"],
        })
    );

    // C030 USpawnBlockerRecorderComponent::RecordOwnerState (0x1418e9500 / 0x1418e9830)
    // and UpdateOwnerToDatabaseState (0x1418f7930 / 0x1418f81f0).
    assert_eq!(
        evaluate_spawn_blocker_transfer(false, 1500, [1.0, 2.0, 3.0, 4.0], "Blocker_Row_A"),
        None
    );
    assert_eq!(
        evaluate_spawn_blocker_transfer(true, 1500, [1.0, 2.0, 3.0, 4.0], "Blocker_Row_A"),
        Some(SpawnBlockerState {
            effective_radius: 1500,
            blocker_transform: [1.0, 2.0, 3.0, 4.0],
            spawn_blocker_row_name: "Blocker_Row_A",
        })
    );

    // C032 USplineRecorderComponent::RecordOwnerState (0x1418e95f0 / 0x1418e9930),
    // RespawnOwner (0x1418ed690 / 0x1418ee060), and ApplyRecorderTransform (0x1418c5cd0 / 0x1418c7970).
    assert_eq!(evaluate_spline_record(false, "Spline_Net_A", 77), None);
    assert_eq!(
        evaluate_spline_record(true, "Spline_Net_A", 77),
        Some(SplineRecordPlan {
            spline_network_state: "Spline_Net_A",
            unique_spline_id: 77,
        })
    );
    assert_eq!(
        evaluate_spline_recorder_respawn(false, false, true),
        SplineRespawnDisposition::FailedNoActor
    );
    assert_eq!(
        evaluate_spline_recorder_respawn(true, false, true),
        SplineRespawnDisposition::Resolved {
            reused_existing_spline_actor: true,
            apply_spline_network_state: true,
        }
    );
    assert_eq!(
        evaluate_spline_recorder_respawn(false, true, false),
        SplineRespawnDisposition::Resolved {
            reused_existing_spline_actor: false,
            apply_spline_network_state: false,
        }
    );
    let mut spline_pts = [[1.0_f32, 0.0, 0.0], [0.0, 2.0, 3.0]];
    // 90 deg yaw around +Z (`[0, 0, sin(pi/4), cos(pi/4)]`), scale `[2, 3, 4]`, translation `[10, 20, 30]`.
    let q_z90 = [
        0.0_f32,
        0.0,
        std::f32::consts::FRAC_1_SQRT_2,
        std::f32::consts::FRAC_1_SQRT_2,
    ];
    transform_spline_point_positions(q_z90, [10.0, 20.0, 30.0], [2.0, 3.0, 4.0], &mut spline_pts);
    // Point 0: scale(1,0,0) = (2,0,0) -> rot_z90 = (0,2,0) -> + (10,20,30) = (10,22,30).
    assert!((spline_pts[0][0] - 10.0).abs() < 1e-5);
    assert!((spline_pts[0][1] - 22.0).abs() < 1e-5);
    assert!((spline_pts[0][2] - 30.0).abs() < 1e-5);
    // Point 1: scale(0,2,3) = (0,6,12) -> rot_z90 = (-6,0,12) -> + (10,20,30) = (4,20,42).
    assert!((spline_pts[1][0] - 4.0).abs() < 1e-5);
    assert!((spline_pts[1][1] - 20.0).abs() < 1e-5);
    assert!((spline_pts[1][2] - 42.0).abs() < 1e-5);

    // C033 UTreeRecorderComponent::RespawnOwner (0x1418ed770 / 0x1418ee140) and
    // RecordOwnerState (0x1418e9820 / 0x1418e9b60):
    // physics-dynamic trees unregister their recorder during FinishSpawningActor;
    // RecordOwnerState unions +0x330 and +0x308 into a deduplicated TreePrimitiveMask.
    assert!(should_drop_respawned_tree_recorder_on_resave(true));
    assert!(!should_drop_respawned_tree_recorder_on_resave(false));
    assert_eq!(
        evaluate_tree_record(false, "Pine_A", &[1, 2], &[2, 3], 0xdead_beef, true),
        None
    );
    assert_eq!(
        evaluate_tree_record(true, "Pine_A", &[4, 2, 4], &[2, 1, 5], 0xdead_beef, true),
        Some(TreeRecordPlan {
            tree_type: "Pine_A",
            tree_primitive_mask: vec![4, 2, 1, 5],
            random_stream: 0xdead_beef,
            b_is_physics_dynamic: true,
        })
    );
}

#[test]
fn prospect_save_state_migrations_handle_v0_v1_and_v2() {
    assert_eq!(LobbyPrivacy::from_saved(0), Some(LobbyPrivacy::Unknown));
    assert_eq!(LobbyPrivacy::from_saved(1), Some(LobbyPrivacy::FriendsOnly));
    assert_eq!(LobbyPrivacy::from_saved(2), Some(LobbyPrivacy::Private));
    assert_eq!(LobbyPrivacy::from_saved(3), None);

    // Version 2 (all supplied saves): no migration or deprecation runs.
    let mut privacy = LobbyPrivacy::Private;
    let mut id = String::from("active-id");
    let mut dt_key = String::from("Olympus_Main");
    let mut blobs = vec![10_u8, 20_u8];
    migrate_prospect_save_state(
        PROSPECT_SAVE_STATE_CURRENT_VERSION,
        &mut privacy,
        &mut id,
        &mut dt_key,
        "legacy-id",
        "legacy-dt",
        &mut blobs,
    );
    assert_eq!(privacy, LobbyPrivacy::Private);
    assert_eq!(id, "active-id");
    assert_eq!(dt_key, "Olympus_Main");
    assert_eq!(blobs, vec![10, 20]);

    // Version 1: copies legacy ProspectID and ProspectDTKey into ProspectInfo, keeps blobs.
    migrate_prospect_save_state(
        1,
        &mut privacy,
        &mut id,
        &mut dt_key,
        "legacy-id",
        "legacy-dt",
        &mut blobs,
    );
    assert_eq!(privacy, LobbyPrivacy::Private);
    assert_eq!(id, "legacy-id");
    assert_eq!(dt_key, "legacy-dt");
    assert_eq!(blobs, vec![10, 20]);

    // Version 0 (NoVersion): deprecates save state, resets ProspectInfo, sets FriendsOnly (1), empties blobs.
    migrate_prospect_save_state(
        0,
        &mut privacy,
        &mut id,
        &mut dt_key,
        "legacy-id",
        "legacy-dt",
        &mut blobs,
    );
    assert_eq!(privacy, LobbyPrivacy::FriendsOnly);
    assert!(id.is_empty());
    assert!(dt_key.is_empty());
    assert!(blobs.is_empty());
}

#[test]
fn actor_recorder_upgrades_tree_primitive_to_wood_only_below_v1() {
    let mut names = vec![
        units("TreePrimitive"),
        units("treeprimitive"),
        units("Stone"),
    ];
    assert_eq!(upgrade_actor_recorder_component_names(3, &mut names), 0);
    assert_eq!(names[0], units("TreePrimitive"));

    assert_eq!(upgrade_actor_recorder_component_names(0, &mut names), 2);
    assert_eq!(
        names,
        vec![units("Wood"), units("Wood"), units("Stone")],
        "version 0 renames TreePrimitive case-insensitively to Wood"
    );
}

#[test]
fn actor_recorder_transform_gate_matches_identity_and_antipodal_quaternion() {
    // Exact identity (passed by ReadFromProspectSaveState) and -Identity quaternion skip ApplyRecorderTransform.
    assert!(!recorder_transform_is_non_identity(
        [0.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0]
    ));
    assert!(!recorder_transform_is_non_identity(
        [0.0, 0.0, 0.0, -1.0],
        [
            ACTOR_RECORDER_TRANSFORM_TOLERANCE,
            -ACTOR_RECORDER_TRANSFORM_TOLERANCE,
            0.0
        ],
        [1.0 + 0.5 * ACTOR_RECORDER_TRANSFORM_TOLERANCE, 1.0, 1.0]
    ));
    // Non-identity translation, rotation, or scale (as in ReadFromInstancedLevelSaveState) triggers slot +0x4b0.
    assert!(recorder_transform_is_non_identity(
        [0.0, 0.0, 0.0, 1.0],
        [1.0, 0.0, 0.0],
        [1.0, 1.0, 1.0]
    ));
    assert!(recorder_transform_is_non_identity(
        [0.0, 0.0, 0.70710677, 0.70710677],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 1.0]
    ));
    assert!(recorder_transform_is_non_identity(
        [0.0, 0.0, 0.0, 1.0],
        [0.0, 0.0, 0.0],
        [1.0, 1.0, 2.0]
    ));
}

#[test]
fn by_name_prospect_normalization_and_id_matching() {
    assert_eq!(normalize_prospect_name(""), None);
    assert_eq!(normalize_prospect_name(".json"), None);
    assert_eq!(normalize_prospect_name(".JSON"), None);
    assert_eq!(normalize_prospect_name("Olympus_Run"), Some("Olympus_Run"));
    assert_eq!(
        normalize_prospect_name("Olympus_Run.json"),
        Some("Olympus_Run")
    );
    assert_eq!(
        normalize_prospect_name("Olympus_Run.JSON"),
        Some("Olympus_Run")
    );
    // Multi-byte UTF-8 characters crossing `len() - 5` must not panic.
    assert_eq!(
        normalize_prospect_name("aa\u{20ac}\u{20ac}"),
        Some("aa\u{20ac}\u{20ac}")
    );
    assert_eq!(
        normalize_prospect_name("aa\u{20ac}\u{20ac}.JSON"),
        Some("aa\u{20ac}\u{20ac}")
    );

    assert!(prospect_ids_match(
        &units("Olympus_Run"),
        &units("olympus_run")
    ));
    assert!(!prospect_ids_match(
        &units("Olympus_Run"),
        &units("Styx_Run")
    ));

    assert!(can_activate_prospect_info("Olympus_Run", ""));
    assert!(!can_activate_prospect_info("", ""));
    assert!(!can_activate_prospect_info("Olympus_Run", "Prior_Run"));
}

#[test]
fn recorder_restore_dispatch_covers_all_38_classes_and_overrides() {
    assert_eq!(recorder_restore_dispatch(0), None);
    assert_eq!(recorder_restore_dispatch(39), None);

    let mut base_subclasses = Vec::new();
    let mut mount_restore_classes = Vec::new();
    let mut grid_transform_classes = Vec::new();
    let mut spline_transform_classes = Vec::new();

    for ordinal in 1..=38 {
        let dispatch = recorder_restore_dispatch(ordinal).expect("valid C001..C038 ordinal");
        assert_eq!(dispatch.class_ordinal, ordinal);
        assert!(dispatch.selected_property_count >= 5);
        if dispatch.version_upgrade == VersionUpgradeKind::NoOp {
            base_subclasses.push(ordinal);
            assert_eq!(dispatch.restore_state, RestoreStateKind::BaseSerializeOnly);
            assert_eq!(dispatch.apply_transform, ApplyTransformKind::NoOpTrue);
        } else {
            assert_eq!(
                dispatch.version_upgrade,
                VersionUpgradeKind::ActorTreePrimitiveToWood
            );
            if dispatch.restore_state == RestoreStateKind::MountActorUprightYaw {
                mount_restore_classes.push(ordinal);
            } else {
                assert_eq!(
                    dispatch.restore_state,
                    RestoreStateKind::ActorWithContextTransform
                );
            }
            match dispatch.apply_transform {
                ApplyTransformKind::ActorAndBuildingGrid => grid_transform_classes.push(ordinal),
                ApplyTransformKind::ActorAndSplinePoints => spline_transform_classes.push(ordinal),
                other => assert_eq!(other, ApplyTransformKind::ActorTransformOnly),
            }
        }
    }

    assert_eq!(base_subclasses, vec![13, 14, 22, 25, 35, 36, 37]);
    assert_eq!(mount_restore_classes, vec![17]);
    assert_eq!(grid_transform_classes, vec![4]);
    assert_eq!(spline_transform_classes, vec![32]);

    // Spot-check selected property counts for C001, C009, C013, C028, C034.
    assert_eq!(
        recorder_restore_dispatch(1)
            .unwrap()
            .selected_property_count,
        20
    );
    assert_eq!(
        recorder_restore_dispatch(9)
            .unwrap()
            .selected_property_count,
        31
    );
    assert_eq!(
        recorder_restore_dispatch(13)
            .unwrap()
            .selected_property_count,
        5
    );
    assert_eq!(
        recorder_restore_dispatch(28)
            .unwrap()
            .selected_property_count,
        23
    );
    assert_eq!(
        recorder_restore_dispatch(34)
            .unwrap()
            .selected_property_count,
        21
    );
}

#[test]
fn entry_path_failure_matrix_evaluates_by_name_and_from_file_outcomes() {
    let id = units("Olympus_Run");
    let other_id = units("Styx_Run");

    // Missing file, empty name, malformed JSON, and name mismatch preserve prior state.
    assert_eq!(
        evaluate_by_name_load_attempt("", true, true, true, &id, false, true, &id),
        ByNameLoadOutcome::RejectedStatePreserved
    );
    assert_eq!(
        evaluate_by_name_load_attempt("Olympus_Run", false, true, true, &id, false, true, &id),
        ByNameLoadOutcome::RejectedStatePreserved
    );
    assert_eq!(
        evaluate_by_name_load_attempt("Olympus_Run", true, false, true, &id, false, true, &id),
        ByNameLoadOutcome::RejectedStatePreserved
    );
    assert_eq!(
        evaluate_by_name_load_attempt("Olympus_Run", true, true, false, &id, false, true, &id),
        ByNameLoadOutcome::RejectedStatePreserved
    );
    assert_eq!(
        evaluate_by_name_load_attempt(
            "Olympus_Run",
            true,
            true,
            true,
            &other_id,
            false,
            true,
            &other_id
        ),
        ByNameLoadOutcome::RejectedStatePreserved
    );

    // Corrupt inner BinaryBlob or inner/outer ID mismatch in TransitionToProspect
    // happens AFTER ResetActiveProspect + SetActiveProspectInfo.
    assert_eq!(
        evaluate_by_name_load_attempt("Olympus_Run.json", true, true, true, &id, false, false, &id),
        ByNameLoadOutcome::AbortedAfterActiveInfoOverwrite
    );
    assert_eq!(
        evaluate_by_name_load_attempt(
            "Olympus_Run.json",
            true,
            true,
            true,
            &id,
            true,
            true,
            &other_id
        ),
        ByNameLoadOutcome::AbortedAfterActiveInfoOverwrite
    );

    // Clean load and changed-during-load with matching IDs.
    assert_eq!(
        evaluate_by_name_load_attempt("Olympus_Run.json", true, true, true, &id, false, true, &id),
        ByNameLoadOutcome::TransitionedToMap { mixed_reads: false }
    );
    assert_eq!(
        evaluate_by_name_load_attempt("Olympus_Run.json", true, true, true, &id, true, true, &id),
        ByNameLoadOutcome::TransitionedToMap { mixed_reads: true }
    );

    // SetupLoadedProspect (0x1414baf60): empty LocalSavePath is a no-op; non-empty
    // LocalSavePath with any reader failure triggers Fatal 0x141ee8930.
    assert_eq!(
        evaluate_setup_loaded_prospect_attempt("", false, false),
        SetupLoadedProspectOutcome::SkippedEmptyLocalSavePath
    );
    assert_eq!(
        evaluate_setup_loaded_prospect_attempt("/tmp/prospect.bin", false, true),
        SetupLoadedProspectOutcome::FatalProcessExit
    );
    assert_eq!(
        evaluate_setup_loaded_prospect_attempt("/tmp/prospect.bin", true, false),
        SetupLoadedProspectOutcome::FatalProcessExit
    );
    assert_eq!(
        evaluate_setup_loaded_prospect_attempt("/tmp/prospect.bin", true, true),
        SetupLoadedProspectOutcome::ActivatedFromBinaryFile
    );
}

#[test]
fn weather_and_teleport_clocks_record_and_restore() {
    // WeatherControllerRecorderComponent (C035) filters active events against CurrentBlockEndTime
    // and records/restores elapsed seconds against UIcarusFunctionLibrary::GetLevelTimeElapsedSec.
    assert!(!should_record_weather_event(-1, 100, 500));
    assert!(should_record_weather_event(120, 100, 220));
    assert!(!should_record_weather_event(121, 100, 220));

    let elapsed = record_weather_event_elapsed(450, 300);
    assert_eq!(elapsed, 150);
    assert_eq!(restored_weather_event_start_time(450, elapsed), 300);
    assert_eq!(restored_weather_event_start_time(0, elapsed), 0);
    assert_eq!(restored_weather_event_start_time(-5, elapsed), 0);

    // C035 UWeatherControllerRecorderComponent::RecordOwnerState (0x1418e98b0 / 0x1418ea8d0)
    // and UpdateOwnerToDatabaseState (0x1418f8120 / 0x1418f87f0).
    let pool_a_events = [
        WeatherControllerEventCandidate {
            event_start_time: 100,
            weather_event_row: "Storm_Light",
            is_row_valid: true,
            row_duration: Some(120),
        },
        WeatherControllerEventCandidate {
            event_start_time: 100,
            weather_event_row: "Storm_Overrun",
            is_row_valid: true,
            row_duration: Some(121),
        },
        WeatherControllerEventCandidate {
            event_start_time: 110,
            weather_event_row: "Storm_Invalid",
            is_row_valid: false,
            row_duration: Some(50),
        },
    ];
    let pool_b_events = [WeatherControllerEventCandidate {
        event_start_time: 150,
        weather_event_row: "Storm_Heavy",
        is_row_valid: true,
        row_duration: Some(60),
    }];
    let pools = [
        ("Pool_Forest", true, pool_a_events.as_slice()),
        ("Pool_Desert_Inactive", false, pool_b_events.as_slice()),
        ("Pool_Arctic", true, pool_b_events.as_slice()),
    ];
    assert_eq!(
        evaluate_weather_controller_record(false, 450, 220, &pools),
        None
    );
    assert_eq!(
        evaluate_weather_controller_record(true, 0, 220, &pools),
        Some(Vec::new())
    );
    assert_eq!(
        evaluate_weather_controller_record(true, 450, -1, &pools),
        Some(Vec::new())
    );
    assert_eq!(
        evaluate_weather_controller_record(true, 450, 220, &pools),
        Some(vec![
            RecordedWeatherEvent {
                weather_event_row: "Storm_Light",
                pool_row: "Pool_Forest",
                time_elapsed: 350,
            },
            RecordedWeatherEvent {
                weather_event_row: "Storm_Heavy",
                pool_row: "Pool_Arctic",
                time_elapsed: 300,
            },
        ])
    );

    let saved_weather_events = [
        SavedWeatherEventEntry {
            weather_event_row: "Storm_Light",
            weather_event_row_valid: true,
            pool_row: "Pool_Forest",
            pool_row_valid: true,
            time_elapsed: 350,
        },
        SavedWeatherEventEntry {
            weather_event_row: "Storm_Skip_Pool",
            weather_event_row_valid: true,
            pool_row: "Pool_Invalid",
            pool_row_valid: false,
            time_elapsed: 100,
        },
        SavedWeatherEventEntry {
            weather_event_row: "Storm_Heavy",
            weather_event_row_valid: true,
            pool_row: "Pool_Arctic",
            pool_row_valid: true,
            time_elapsed: 300,
        },
    ];
    assert_eq!(
        evaluate_weather_controller_restore(false, 450, &saved_weather_events),
        None
    );
    assert_eq!(
        evaluate_weather_controller_restore(true, 0, &saved_weather_events),
        None
    );
    assert_eq!(
        evaluate_weather_controller_restore(true, 450, &saved_weather_events),
        Some(vec![
            ("Pool_Forest", "Storm_Light", 100),
            ("Pool_Arctic", "Storm_Heavy", 150),
        ])
    );

    // BaseLevelTeleportRecorderComponent (C002) uses GetReplicatedProspectGameTime = GetTimeSeconds + *(float*)(GameState + 0x398).
    let now = replicated_prospect_game_time(42.5, 1000.0);
    assert_eq!(now, 1042.5);
    assert_eq!(remaining_cooldown(1060.9, now), 18);

    // C002 UBaseLevelTeleportRecorderComponent::RecordOwnerState (0x1419d5a20 / 0x1419ccd70)
    // and UpdateOwnerToDatabaseState (0x1419e27c0 / 0x1419d9a70).
    assert_eq!(
        evaluate_base_level_teleport_record(false, true, 1060.9, true, 42.5, 1000.0),
        None
    );
    assert_eq!(
        evaluate_base_level_teleport_record(true, true, 1060.9, true, 42.5, 1000.0),
        Some(BaseLevelTeleportRecordPlan {
            b_is_activated: true,
            remaining_cooldown: 18,
        })
    );
    assert_eq!(
        evaluate_base_level_teleport_record(true, false, 1060.9, false, 42.5, 1000.0),
        Some(BaseLevelTeleportRecordPlan {
            b_is_activated: false,
            remaining_cooldown: 0,
        })
    );
    assert_eq!(evaluate_base_level_teleport_restore(false, true, 18), None);
    assert_eq!(
        evaluate_base_level_teleport_restore(true, true, 18),
        Some(BaseLevelTeleportRestorePlan {
            b_is_activated: true,
            notify_activated_and_clear_timer: true,
            start_cooldown_seconds: 18,
        })
    );
}

#[test]
fn d01_dynamic_item_properties_overflow_modifiers_and_variables() {
    assert_eq!(
        DynamicItemPropertyKind::from_saved_index(0),
        Some(DynamicItemPropertyKind::AssociatedItemInventoryId)
    );
    assert_eq!(
        DynamicItemPropertyKind::from_saved_index(3),
        Some(DynamicItemPropertyKind::GunCurrentMagSize)
    );
    assert_eq!(
        DynamicItemPropertyKind::from_saved_index(4),
        Some(DynamicItemPropertyKind::CurrentAmmoType)
    );
    assert_eq!(
        DynamicItemPropertyKind::from_saved_index(7),
        Some(DynamicItemPropertyKind::ItemableStack)
    );
    assert_eq!(
        DynamicItemPropertyKind::from_saved_index(13),
        Some(DynamicItemPropertyKind::InventoryContainerLinkedInventoryId)
    );
    assert_eq!(DynamicItemPropertyKind::from_saved_index(14), None);
    // Low-byte truncation in UpdateOwnerInventories (0x1418b8f20): 256 -> u8 0.
    assert_eq!(
        DynamicItemPropertyKind::from_saved_index(256),
        Some(DynamicItemPropertyKind::AssociatedItemInventoryId)
    );

    // C001 UActorStateRecorderComponent::PerformItemVersionUpgrades (0x14189a250 / 0x1418a3530):
    // upgrades gun ammo into linked inventory slot 0 when ActorStateRecorderVersion < 2.
    assert_eq!(
        evaluate_legacy_gun_ammo_upgrade(2, true, true, &[(4, 2), (3, 8), (13, 42)]),
        None
    );
    assert_eq!(
        evaluate_legacy_gun_ammo_upgrade(1, false, true, &[(4, 2), (3, 8), (13, 42)]),
        None
    );
    assert_eq!(
        evaluate_legacy_gun_ammo_upgrade(1, true, false, &[(4, 2), (3, 8), (13, 42)]),
        None
    );
    assert_eq!(
        evaluate_legacy_gun_ammo_upgrade(1, true, true, &[(4, 0), (3, 8)]),
        None
    );
    assert_eq!(
        evaluate_legacy_gun_ammo_upgrade(1, true, true, &[(4, 2), (3, 0)]),
        None
    );
    assert_eq!(
        evaluate_legacy_gun_ammo_upgrade(1, true, true, &[(4, 2), (3, 8)]),
        Some(LegacyGunAmmoUpgrade {
            ammo_type_index: 1,
            ammo_stack_count: 8,
            existing_linked_inventory_id: None,
        })
    );
    assert_eq!(
        evaluate_legacy_gun_ammo_upgrade(1, true, true, &[(4, 1), (3, 30), (13, 77)]),
        Some(LegacyGunAmmoUpgrade {
            ammo_type_index: 0,
            ammo_stack_count: 30,
            existing_linked_inventory_id: Some(77),
        })
    );

    assert_eq!(
        resolve_inventory_slot_restore(true, 0),
        InventorySlotOverflowDisposition::PlacedInSlot
    );
    assert_eq!(
        resolve_inventory_slot_restore(false, 0),
        InventorySlotOverflowDisposition::ForceAddItemsWhenEmpty
    );
    assert_eq!(
        resolve_inventory_slot_restore(false, 8),
        InventorySlotOverflowDisposition::AppendToDeferredOverflow
    );

    assert_eq!(
        normalize_restored_modifier_lifetime(-1),
        RESTORED_INDEFINITE_MODIFIER_LIFETIME
    );
    assert_eq!(normalize_restored_modifier_lifetime(45), 45);
    assert_eq!(
        evaluate_modifier_state_restore(false, 1.0, true, true, -1),
        None
    );
    assert_eq!(
        evaluate_modifier_state_restore(true, -0.1, true, true, -1),
        None
    );
    assert_eq!(
        evaluate_modifier_state_restore(true, f32::NAN, true, true, -1),
        None
    );
    assert_eq!(
        evaluate_modifier_state_restore(true, 0.0, false, true, -1),
        None
    );
    assert_eq!(
        evaluate_modifier_state_restore(true, 0.0, true, false, -1),
        None
    );
    assert_eq!(
        evaluate_modifier_state_restore(true, 0.0, true, true, -1),
        Some(RESTORED_INDEFINITE_MODIFIER_LIFETIME)
    );
    assert_eq!(
        evaluate_modifier_state_restore(true, 1.5, true, true, 60),
        Some(60)
    );

    let mut vars = Vec::new();
    assert!(upsert_recorded_variable(&mut vars, "bIsActive", 1_i32));
    assert!(upsert_recorded_variable(&mut vars, "FuelCount", 10_i32));
    assert!(!upsert_recorded_variable(&mut vars, "bIsActive", 0_i32));
    assert_eq!(vars, vec![("bIsActive", 0), ("FuelCount", 10)]);
}

#[test]
fn prospect_blob_decode_models_allocation_lengths_and_ignored_base64_return() {
    // 1. TotalLength < 0 invokes TArray::ResizeShrink on an empty array before FBase64::Decode (SL-003).
    assert_eq!(
        evaluate_prospect_blob_decode(-1, 512, 1024, 512, true, true, true),
        ProspectBlobDecodeOutcome::NegativeTotalLengthResizeError
    );

    // 2. UncompressedLength < 0 skips ResizeGrow and calls memset(nullptr, 0, ...) even when
    // FBase64::Decode returned false (SL-001, SL-003).
    assert_eq!(
        evaluate_prospect_blob_decode(512, 512, -1, 0, false, false, false),
        ProspectBlobDecodeOutcome::NegativeUncompressedLengthNullMemsetCrash
    );

    // 3. FBase64::Decode resizes CompressedData to GetDecodedDataSize(BinaryBlob); when smaller than
    // TotalLength (including 0 when BinaryBlob is empty or length <= 3), UncompressMemory reads
    // TotalLength bytes from the shrunk/null buffer regardless of base64_decode_ok (SL-001, SL-004).
    assert_eq!(
        evaluate_prospect_blob_decode(512, 512, 1024, 0, false, false, false),
        ProspectBlobDecodeOutcome::CompressedBufferUnderflowOutOfBoundsRead {
            allocated_bytes: 0,
            uncompress_read_bytes: 512,
        }
    );
    assert_eq!(
        evaluate_prospect_blob_decode(512, 512, 1024, 256, true, true, true),
        ProspectBlobDecodeOutcome::CompressedBufferUnderflowOutOfBoundsRead {
            allocated_bytes: 256,
            uncompress_read_bytes: 512,
        }
    );

    // 4. When decoded_base64_bytes >= TotalLength, corrupt base64 or corrupt zlib fails cleanly in
    // FCompression::UncompressMemory(NAME_Zlib) (SL-001, SL-002).
    assert_eq!(
        evaluate_prospect_blob_decode(512, 512, 1024, 512, false, false, true),
        ProspectBlobDecodeOutcome::ZlibUncompressRejected {
            base64_decode_ok: false,
        }
    );
    assert_eq!(
        evaluate_prospect_blob_decode(512, 512, 1024, 512, true, false, true),
        ProspectBlobDecodeOutcome::ZlibUncompressRejected {
            base64_decode_ok: true,
        }
    );

    // 5. Zlib uncompress succeeds, but FSHA1::HashBuffer does not match Blob.Hash (+0x10).
    assert_eq!(
        evaluate_prospect_blob_decode(512, 512, 1024, 512, true, true, false),
        ProspectBlobDecodeOutcome::Sha1Mismatch
    );

    // 6. Clean decode and deserialize: DataLength (+0x24) is never read by ReadFromProspectBlob
    // and does not affect the outcome even when it differs from TotalLength.
    assert_eq!(
        evaluate_prospect_blob_decode(512, 512, 1024, 512, true, true, true),
        ProspectBlobDecodeOutcome::DecodedSaveState {
            ignored_data_length: 512,
        }
    );
    assert_eq!(
        evaluate_prospect_blob_decode(512, -999, 1024, 600, true, true, true),
        ProspectBlobDecodeOutcome::DecodedSaveState {
            ignored_data_length: -999,
        }
    );
}
