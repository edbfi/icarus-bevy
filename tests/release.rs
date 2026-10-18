use std::path::Path;

use icarus_bevy::experimental::{
    peer_lab::PeerLabConfig,
    release::{
        ReleaseClassNetCacheBounds, ReleasePin, ReleaseProfile, ReleaseRowHandleProfile,
        ReleaseTagProfile, TAG_NET_INDEX_HASH_3_0_31, TAG_NET_INDEX_HASH_3_0_32, network_checksum,
    },
    rep_layout::{ICARUS_ROW_HANDLE_NONE, TagEncoding, icarus_row_handle_width_bytes},
};

#[test]
fn checksums_match_the_logged_formula() {
    // 1.0.0 is observed in both builds' logs; the others follow the same rule.
    assert_eq!(network_checksum("1.0.0"), 1_447_163_286);
    assert_eq!(
        network_checksum("3.0.30.158174-rel-DangerousHorizons"),
        1_379_612_455
    );
    assert_eq!(
        network_checksum("3.0.31.158478-rel-DangerousHorizons"),
        2_406_501_229
    );
    assert_eq!(
        network_checksum("3.0.32.158743-rel-DangerousHorizons"),
        243_431_684
    );
}

#[test]
fn the_tracked_pin_is_consistent() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml");
    let pin = ReleasePin::load(&path).unwrap();
    assert_eq!(pin.app_id, 2_089_300);
    assert_eq!(pin.network_checksum, network_checksum(&pin.project_version));

    let profile = ReleaseProfile::load(&path).unwrap();
    assert_eq!(profile, ReleaseProfile::release_3_0_32());
}

#[test]
fn builtin_profiles_match_observed_3_0_31_and_3_0_32_metadata() {
    let p31 = ReleaseProfile::release_3_0_31();
    assert!(p31.validate().is_ok());
    assert_eq!(p31.pin.build_id, 25_793_607);
    assert_eq!(p31.pin.network_checksum, 2_406_501_229);
    assert_eq!(p31.tags.data_table_tag_count, 130);
    assert_eq!(p31.tags.ini_tag_count, 1_886);
    assert_eq!(p31.tags.unique_node_count, 2_299);
    assert_eq!(p31.tags.total_bits, 12);
    assert_eq!(p31.tags.container_bits, 6);
    assert_eq!(p31.tags.net_index_hash, TAG_NET_INDEX_HASH_3_0_31);
    assert_eq!(
        p31.tags.tag_encoding(),
        TagEncoding::NetIndex {
            first_segment: 0,
            total_bits: 12,
        }
    );

    let p32 = ReleaseProfile::release_3_0_32();
    assert!(p32.validate().is_ok());
    assert_eq!(p32.pin.build_id, 25_956_137);
    assert_eq!(p32.pin.network_checksum, 243_431_684);
    assert_eq!(p32.tags.data_table_tag_count, 130);
    assert_eq!(p32.tags.ini_tag_count, 1_889);
    assert_eq!(p32.tags.unique_node_count, 2_302);
    assert_eq!(p32.tags.net_index_hash, TAG_NET_INDEX_HASH_3_0_32);

    assert_eq!(
        p32.class_net_cache.actor_bound("BP_IcarusPlayerState_C"),
        Some(38)
    );
    assert_eq!(p32.class_net_cache.actor_bound("BP_Mount_Base_C"), Some(84));
    assert_eq!(
        p32.class_net_cache.actor_bound("BP_Mount_Horse_C"),
        Some(85)
    );
    assert_eq!(p32.class_net_cache.actor_bound("BP_Tame_Base_C"), Some(86));
    assert_eq!(
        p32.class_net_cache
            .actor_bound("AIcarusPlayerControllerSurvival"),
        Some(183)
    );
    assert_eq!(p32.class_net_cache.actor_bound("AActor"), None);

    for index in 0..p32.row_handles.table_count {
        assert_eq!(
            p32.row_handles.row_width_bytes(index).unwrap(),
            icarus_row_handle_width_bytes(index).unwrap()
        );
    }
    assert_eq!(
        p32.row_handles
            .row_width_bytes(ICARUS_ROW_HANDLE_NONE)
            .unwrap(),
        0
    );
    assert!(
        p32.row_handles
            .row_width_bytes(p32.row_handles.table_count)
            .is_err()
    );

    let lab = PeerLabConfig::default().with_release_profile(&p32);
    assert_eq!(lab.local_network_version, Some(243_431_684));
}

#[test]
fn explicit_toml_profile_sections_parse_and_validate() {
    let toml_text = r#"
[release]
app_id = 2089300
build_id = 25956137
project_version = "3.0.32.158743-rel-DangerousHorizons"
network_checksum = 243431684

[tags]
data_table_tag_count = 130
ini_tag_count = 1889
unique_node_count = 2302
total_bits = 12
configured_first_segment = 16
effective_first_segment = 0
container_bits = 6
net_index_hash = 1829726063

[class_net_cache]
stat_container = 5
inventory = 11
ui_projection_mount_status = 7
bp_icarus_player_state = 38
bp_mount_base = 84
bp_mount_horse = 85
bp_tame_base = 86
bp_icarus_player_controller_survival = 183

[row_handles]
table_count = 293
zero_row_tables = [62, 164, 181]
two_byte_row_tables = [0, 29, 54, 64, 65, 73, 80, 84, 92, 112, 134, 135, 136, 148, 152, 177, 178, 201, 207, 209, 215, 230, 234, 237, 248, 252, 254, 278]
"#;
    let parsed = ReleaseProfile::parse(toml_text).unwrap();
    assert_eq!(parsed, ReleaseProfile::release_3_0_32());
}

#[test]
fn invalid_profile_bounds_and_tables_fail_closed() {
    let mut bad_tags = ReleaseTagProfile {
        total_bits: 11,
        ..ReleaseProfile::release_3_0_32().tags
    };
    assert!(bad_tags.validate().is_err());
    bad_tags.total_bits = 13;
    assert!(bad_tags.validate().is_err());

    let bad_bounds = ReleaseClassNetCacheBounds {
        stat_container: 0,
        ..ReleaseClassNetCacheBounds::default()
    };
    assert!(bad_bounds.validate().is_err());

    let bad_rows = ReleaseRowHandleProfile {
        zero_row_tables: vec![164, 62],
        ..ReleaseRowHandleProfile::default()
    };
    assert!(bad_rows.validate().is_err());

    let overlap_rows = ReleaseRowHandleProfile {
        zero_row_tables: vec![0, 62],
        ..ReleaseRowHandleProfile::default()
    };
    assert!(overlap_rows.validate().is_err());

    let mut out_of_range_rows = ReleaseRowHandleProfile::default();
    out_of_range_rows.two_byte_row_tables.push(293);
    assert!(out_of_range_rows.validate().is_err());
}

#[test]
fn a_stale_checksum_or_unknown_field_is_rejected() {
    let good = "[release]\napp_id = 2089300\nbuild_id = 1\nproject_version = \"1.0.0\"\nnetwork_checksum = 1447163286\n";
    assert!(ReleasePin::parse(good).is_ok());
    assert!(
        ReleaseProfile::parse(good)
            .unwrap_err()
            .to_string()
            .contains("no built-in tag profile for build_id 1")
    );
    let stale = good.replace("1447163286", "1447163287");
    assert!(
        ReleasePin::parse(&stale)
            .unwrap_err()
            .to_string()
            .contains("does not match")
    );
    assert!(ReleasePin::parse(&format!("{good}extra = 1\n")).is_err());
    assert!(ReleasePin::parse(&good.replace("\"1.0.0\"", "\"\"")).is_err());
}
