use icarus_bevy::experimental::restore_rules::{
    ACTOR_RECORDER_TRANSFORM_TOLERANCE, ActorLink, ApplyTransformKind, AttachRecorderOutcome,
    ByNameLoadOutcome, DynamicItemPropertyKind, InventorySlotOverflowDisposition, LobbyPrivacy,
    OwnerResolvePolicy, OwnerResolveStep, PROSPECT_SAVE_STATE_CURRENT_VERSION, PlayerList,
    ProspectAssociatedMember, ProspectBlobDecodeOutcome, RESTORED_INDEFINITE_MODIFIER_LIFETIME,
    RestoreStateKind, RocketRestore, SetupLoadedProspectOutcome, VOXEL_LOCATION_TOLERANCE,
    VersionUpgradeKind, can_activate_prospect_info, container_entries,
    does_actor_recorder_match_actor, does_player_recorder_match_actor,
    does_player_state_recorder_match_actor, does_state_recorder_match_actor,
    does_voxel_recorder_match_actor, evaluate_attach_recorder_to_actor,
    evaluate_by_name_load_attempt, evaluate_owner_resolve_step, evaluate_prospect_blob_decode,
    evaluate_setup_loaded_prospect_attempt, migrate_prospect_save_state, names_match,
    normalize_prospect_name, normalize_restored_modifier_lifetime, player_record_valid,
    prospect_ids_match, record_weather_event_elapsed, recorded_level, recorder_restore_dispatch,
    recorder_transform_is_non_identity, remaining_cooldown, replicated_prospect_game_time,
    resolve_cave_ai_class_name, resolve_inventory_slot_restore,
    resolve_item_recorder_database_guid, resolve_prospect_player_list,
    resource_deposit_record_valid, restored_elapsed_seconds, restored_weather_event_start_time,
    rocket_record_valid, rocket_restore, should_auto_activate_respawned_drill,
    should_record_weather_event, should_restore_actor_transform,
    should_try_fast_path_prebuilt_grid_restore, stitch, stitch_trapped_character_guids,
    upgrade_actor_recorder_component_names, upright_rotation, upsert_recorded_variable,
    validate_rocket_spawn_record, voxel_location_matches, voxel_record_valid,
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
}

#[test]
fn unrecorded_levels_are_skipped() {
    assert_eq!(recorded_level(-1), None);
    assert_eq!(recorded_level(0), Some(0));
    assert_eq!(recorded_level(-2), Some(-2));
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

    // C011 URocketSpawnRecorderComponent::Validate resets PlayerID and ChrSlot on version < 1
    // or Active { has_player: false }, while keeping the spawn record valid (returns true).
    let mut spawn_player = steam_id.clone();
    let mut spawn_slot = 0;
    assert!(validate_rocket_spawn_record(
        1,
        PlayerList::Active { has_player: true },
        &mut spawn_player,
        &mut spawn_slot,
    ));
    assert_eq!(spawn_player, steam_id);
    assert_eq!(spawn_slot, 0);

    assert!(validate_rocket_spawn_record(
        1,
        PlayerList::Active { has_player: false },
        &mut spawn_player,
        &mut spawn_slot,
    ));
    assert!(spawn_player.is_empty());
    assert_eq!(spawn_slot, -1);

    let mut legacy_player = steam_id.clone();
    let mut legacy_slot = 3;
    assert!(validate_rocket_spawn_record(
        0,
        PlayerList::Active { has_player: true },
        &mut legacy_player,
        &mut legacy_slot,
    ));
    assert!(legacy_player.is_empty());
    assert_eq!(legacy_slot, -1);

    // C028 UResourceDepositRecorderComponent::Validate rejects "Super_Cooled_Ice" case-insensitively.
    assert!(!resource_deposit_record_valid(&units("Super_Cooled_Ice")));
    assert!(!resource_deposit_record_valid(&units("super_cooled_ice")));
    assert!(resource_deposit_record_valid(&units("Iron_Deep_Ore")));
    assert!(resource_deposit_record_valid(&units("Exotic_Red")));
}

#[test]
fn container_positions_keep_gaps() {
    let list = [Some('a'), None, Some('c'), None];
    let entries: Vec<_> = container_entries(&list).collect();
    assert_eq!(entries, [(0, &'a'), (2, &'c')]);
    assert_eq!(container_entries::<u8>(&[]).count(), 0);
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

    // C010 UDrillRecorderComponent::RespawnOwner (0x1418a74f0 / 0x1418abfd0).
    assert!(should_auto_activate_respawned_drill(true, true, true));
    assert!(!should_auto_activate_respawned_drill(true, true, false));
    assert!(!should_auto_activate_respawned_drill(true, false, true));
    assert!(!should_auto_activate_respawned_drill(false, true, true));

    // C004 UBuildingGridRecorderComponent::UpdateOwnerToDatabaseState (0x1418b7770 / 0x1418ba640).
    assert!(should_try_fast_path_prebuilt_grid_restore(true, true));
    assert!(!should_try_fast_path_prebuilt_grid_restore(true, false));
    assert!(!should_try_fast_path_prebuilt_grid_restore(false, true));

    // C021 UItemStateRecorderComponent::RespawnOwner (0x1418a7660 / 0x1418ac140).
    assert_eq!(resolve_item_recorder_database_guid(55, 77), 77);
    assert_eq!(resolve_item_recorder_database_guid(0, 77), -1);
    assert_eq!(resolve_item_recorder_database_guid(-5, 77), -1);

    // C001/C023 UActorStateRecorderComponent::UpdateOwnerToDatabaseState (0x1418b6ed0 / 0x1418ba040).
    assert!(should_restore_actor_transform(true));
    assert!(!should_restore_actor_transform(false));
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

    // BaseLevelTeleportRecorderComponent (C002) uses GetReplicatedProspectGameTime = GetTimeSeconds + *(float*)(GameState + 0x398).
    let now = replicated_prospect_game_time(42.5, 1000.0);
    assert_eq!(now, 1042.5);
    assert_eq!(remaining_cooldown(1060.9, now), 18);
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
