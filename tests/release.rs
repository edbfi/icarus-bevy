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

#[test]
fn builtin_player_controller_profiles_compose_the_captured_export_prefix() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits},
        archive::ByteOrder,
        session_open::{ConnectionExports, NetGuidCache, PathSegment, encode_exports},
    };
    use sha2::{Digest, Sha256};
    let p31 = ReleaseProfile::release_3_0_31().player_controller.unwrap();
    assert_eq!(
        ReleaseProfile::release_3_0_32().player_controller.as_ref(),
        Some(&p31)
    );
    assert!(p31.validate().is_ok());
    // The capture's map chain (F-778); the level is the caller's, not release data.
    let level = vec![
        PathSegment::new("PersistentLevel", 0, true),
        PathSegment::new("Terrain_016", 0, true),
        PathSegment::new("/Game/Maps/Terrain_016_OLY/Terrain_016", 0, true),
    ];
    let profile = p31.session_profile(level);
    let mut cache = NetGuidCache::new();
    cache.register_level(&profile.level);
    let (guids, references) = cache
        .player_controller_exports(&mut ConnectionExports::new(), &profile)
        .unwrap();
    assert_eq!(guids.components, vec![6, 8, 10, 12, 14, 16, 18]);
    let codec = ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    // F-778/F-779: the captured first part's export prefix is these 3,793
    // bits (SHA-256 of its 475 bytes in the M-707 capture, pinned by M-819),
    // so any wrong path, flag or checksum in the built-in identifiers fails.
    let payload = encode_exports(&references, &codec).unwrap();
    assert_eq!(payload.bit_len(), 3_793);
    let digest: String = Sha256::digest(payload.bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        digest,
        "927793245d9f2a5801c56ba40acb482570729030d5f53643760c4a12a7c02399"
    );
}

#[test]
fn player_controller_sections_parse_and_fail_closed() {
    let release = "[release]\napp_id = 2089300\nbuild_id = 25956137\nproject_version = \"3.0.32.158743-rel-DangerousHorizons\"\nnetwork_checksum = 243431684\n";
    let section = r#"
[player_controller]
archetype = [
    { path = "Default__Test_C", checksum = 7 },
    { path = "/Game/Test", checksum = 0 },
]
components = [
    { name = "A", checksum = 1, block = "rep_bools", rep_bools = [[2, true]] },
    { name = "B", checksum = 2 },
    { name = "C", checksum = 3, block = "create" },
]
"#;
    let parsed = ReleaseProfile::parse(&format!("{release}{section}")).unwrap();
    let player_controller = parsed.player_controller.unwrap();
    assert_eq!(player_controller.components[0].rep_bools, vec![(2, true)]);
    assert_ne!(
        Some(player_controller),
        ReleaseProfile::release_3_0_32().player_controller
    );
    // A known build without the section gets its built-in identifiers; an
    // unknown build gets none rather than an older build's.
    assert_eq!(
        ReleaseProfile::parse(release).unwrap().player_controller,
        ReleaseProfile::release_3_0_32().player_controller
    );
    let unknown = release
        .replace("25956137", "1")
        .replace("3.0.32.158743-rel-DangerousHorizons", "1.0.0")
        .replace("243431684", "1447163286");
    let tags = "[tags]\ndata_table_tag_count = 130\nini_tag_count = 1889\nunique_node_count = 2302\ntotal_bits = 12\nconfigured_first_segment = 16\neffective_first_segment = 0\ncontainer_bits = 6\nnet_index_hash = 1829726063\n";
    assert_eq!(
        ReleaseProfile::parse(&format!("{unknown}{tags}"))
            .unwrap()
            .player_controller,
        None
    );
    for bad in [
        section.replace(
            "block = \"create\"",
            "block = \"create\", rep_bools = [[1, true]]",
        ),
        section.replace("[[2, true]]", "[[3, true]]"),
        section.replace("[[2, true]]", "[[2, true], [1, false]]"),
        section.replace("[[2, true]]", "[]"),
        section.replace("name = \"B\"", "name = \"a\""),
        section.replace("checksum = 7", "checksum = 0"),
        section.replace("checksum = 0 }", "checksum = 5 }"),
        section.replace(
            "    { name = \"C\", checksum = 3, block = \"create\" },\n",
            "",
        ),
        section.replace("block = \"create\"", "block = \"spawn\""),
        section.replace("checksum = 2 }", "checksum = 2, extra = 1 }"),
    ] {
        assert_ne!(bad, section);
        assert!(
            ReleaseProfile::parse(&format!("{release}{bad}")).is_err(),
            "{bad}"
        );
    }
}

#[test]
fn builtin_player_state_identifiers_compose_the_captured_export_prefix() {
    use icarus_bevy::experimental::{
        archive::ByteOrder,
        control_messages::{ArchiveText, NetIdType, NetIdValue, UniqueNetId},
        session_open::{
            ConnectionExports, NetGuidCache, PathSegment, PlayerStateOpenState,
            compose_player_state_open,
        },
    };
    use sha2::{Digest, Sha256};
    let p31 = ReleaseProfile::release_3_0_31();
    assert_eq!(
        ReleaseProfile::release_3_0_32().player_state,
        p31.player_state
    );
    let plan = p31
        .player_state_open_plan("/Game/Maps/Terrain_016_OLY/Terrain_016")
        .unwrap()
        .unwrap();
    // The capture had exported the level and 16 other static paths to this
    // connection first, so the archetype took GUID 41 (F-784).
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    cache.export_static(&mut exports, &plan.level).unwrap();
    for index in 0..16 {
        cache
            .export_static(
                &mut exports,
                &[PathSegment::new(&format!("other{index}"), 0, false)],
            )
            .unwrap();
    }
    // The client had acked them (F-796).
    exports.commit_bunch(1);
    exports.received_ack(1);
    // The capture's values: the authored probe's synthetic Login id and name
    // (`examples/handshake_probe.rs`), PlayerId 256, owner 2, host.
    let state = PlayerStateOpenState {
        player_state: 4,
        owner: 2,
        player_id: 256,
        unique_id: UniqueNetId {
            kind: NetIdType::Indexed(1),
            value: NetIdValue::Text(ArchiveText::text("IcarusProbe-0001").unwrap()),
        },
        player_name: ArchiveText::text("IcarusProbe").unwrap(),
        is_host: true,
    };
    let open =
        compose_player_state_open(&mut cache, &mut exports, &plan, &state, ByteOrder::Little)
            .unwrap();
    let hex_digest = |bytes: &[u8]| -> String {
        Sha256::digest(bytes)
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    };
    // SHA-256 of the capture's 96-byte, 761-bit PlayerState export prefix and
    // of its 58-byte, 462-bit body (M-823).
    let payload = open.exports.unwrap();
    assert_eq!(payload.bit_len(), 761);
    assert_eq!(
        hex_digest(payload.bytes()),
        "f65a6aea3492ebf6b3082a530a4adfd8379634320f1201f059d3ce1b6e46a9df"
    );
    assert_eq!(open.bunch.payload.bit_len(), 462);
    assert_eq!(
        hex_digest(open.bunch.payload.bytes()),
        "531bb7b3e9a59fa4f7423e5718a48238ffb93219c32dafb96f4826459daebd09"
    );
}

#[test]
fn builtin_game_state_identifiers_compose_the_captured_export_prefix() {
    use icarus_bevy::experimental::{
        archive::ByteOrder,
        game_state_open::{GameStateManagers, GameStateOpenState, compose_game_state_open},
        session_open::{ActorKey, ConnectionExports, NetGuidCache, PathSegment},
    };
    use sha2::{Digest, Sha256};
    let p31 = ReleaseProfile::release_3_0_31();
    assert_eq!(ReleaseProfile::release_3_0_32().game_state, p31.game_state);
    let plan = p31
        .game_state_open_plan("/Game/Maps/Terrain_016_OLY/Terrain_016")
        .unwrap()
        .unwrap();
    // The capture had exported the level and 8 other static paths and
    // assigned 33 dynamic GUIDs first, so the GameState took 68 and its
    // archetype 25 (F-794).
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    cache.export_static(&mut exports, &plan.level).unwrap();
    for index in 0..8 {
        cache
            .export_static(
                &mut exports,
                &[PathSegment::new(&format!("other{index}"), 0, false)],
            )
            .unwrap();
    }
    for _ in 0..33 {
        cache.assign_dynamic();
    }
    // The client had acked them (F-796).
    exports.commit_bunch(1);
    exports.received_ack(1);
    // All seven managers are named, so the weather component takes 88.
    let manager = |id| Some(ActorKey::World(id));
    let state = GameStateOpenState {
        key: ActorKey::Session(0),
        managers: GameStateManagers {
            quest_manager: manager(1),
            world_talent_manager: manager(2),
            radiation_manager: manager(3),
            inventory_container_manager: manager(4),
            player_history_tracker: manager(5),
            flod: manager(6),
            disaster_controller: manager(7),
        },
        values: std::collections::BTreeMap::new(),
        weather: Vec::new(),
    };
    let open = compose_game_state_open(
        &mut cache,
        &mut exports,
        &plan,
        &state,
        4,
        ByteOrder::Little,
    )
    .unwrap();
    assert_eq!(cache.actor_guid(ActorKey::Session(0)), 68);
    assert_eq!(cache.actor_guid(ActorKey::World(6)), 84);
    // SHA-256 of the capture's 298-byte, 2,377-bit GameState export prefix.
    let payload = open.exports.unwrap();
    assert_eq!(payload.bit_len(), 2_377);
    let digest: String = Sha256::digest(payload.bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    assert_eq!(
        digest,
        "7a403039925761dec0575e9e96c396321337399a17ac7c2f38c041da1e75f767"
    );
}

#[test]
fn game_state_sections_parse_and_fail_closed() {
    let release = "[release]\napp_id = 2089300\nbuild_id = 1\nproject_version = \"1.0\"\n";
    let release = release.replace(
        "\"1.0\"\n",
        &format!(
            "\"1.0\"\nnetwork_checksum = {}\n",
            icarus_bevy::experimental::release::network_checksum("1.0")
        ),
    );
    let tags = "[tags]\ndata_table_tag_count = 130\nini_tag_count = 1889\nunique_node_count = 2302\ntotal_bits = 12\nconfigured_first_segment = 16\neffective_first_segment = 0\ncontainer_bits = 6\nnet_index_hash = 1829726063\n";
    let section = "[game_state]\n\
        archetype = [{ path = \"Default__G_C\", checksum = 1 }, { path = \"/Game/G\", checksum = 0 }]\n\
        game_mode_class = [{ path = \"M_C\", checksum = 2 }, { path = \"/Game/M\", checksum = 0 }]\n\
        spectator_class = [{ path = \"S\", checksum = 3 }, { path = \"/Script/E\", checksum = 0 }]\n\
        connected_players = { name = \"A\", checksum = 4 }\n\
        dialogue_system = { name = \"B\", checksum = 5 }\n\
        weather = { name = \"C\", checksum = 6 }\n";
    let base = format!("{release}{tags}");
    // Unknown builds get no built-in identifiers.
    assert!(ReleaseProfile::parse(&base).unwrap().game_state.is_none());
    let parsed = ReleaseProfile::parse(&format!("{base}{section}")).unwrap();
    let plan = parsed
        .game_state_open_plan("/Game/Maps/Test/Test")
        .unwrap()
        .unwrap();
    assert_eq!(plan.components.weather.name, "C");
    assert_eq!(plan.spectator_class[1].path, "/Script/E");
    for bad in [
        section.replace("checksum = 4", "checksum = 0"),
        section.replace("name = \"B\"", "name = \"a\""),
        section.replace("name = \"C\"", "name = \"\""),
        section.replace(
            "{ path = \"M_C\", checksum = 2 }",
            "{ path = \"M_C\", checksum = 0 }",
        ),
        section.replace(", { path = \"/Script/E\", checksum = 0 }", ""),
        section.replace("checksum = 6 }", "checksum = 6, extra = 1 }"),
    ] {
        assert_ne!(bad, section);
        assert!(
            ReleaseProfile::parse(&format!("{base}{bad}")).is_err(),
            "{bad}"
        );
    }
}
