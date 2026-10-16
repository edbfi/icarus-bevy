use icarus_bevy::experimental::restore_rules::{
    ActorLink, OwnerResolvePolicy, PlayerList, RocketRestore, VOXEL_LOCATION_TOLERANCE,
    container_entries, names_match, player_record_valid, recorded_level, remaining_cooldown,
    restored_elapsed_seconds, rocket_record_valid, rocket_restore, stitch, upright_rotation,
    voxel_location_matches, voxel_record_valid,
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
    assert!(names_match(&units("Größe"), &units("GRößE")));
    assert!(!names_match(&units("é"), &units("É")));
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
