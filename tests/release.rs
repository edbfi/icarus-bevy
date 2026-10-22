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

#[test]
fn builtin_pawn_identifiers_compose_the_archetype_and_components() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, PackageMapState},
        archive::ByteOrder,
        pawn_open::{PawnContent, PawnOpenState, compose_pawn_open},
        session_open::{ActorKey, ConnectionExports, NetGuidCache},
    };
    let p31 = ReleaseProfile::release_3_0_31();
    assert!(p31.pawn.is_some());
    assert_eq!(ReleaseProfile::release_3_0_32().pawn, p31.pawn);
    let plan = p31
        .pawn_open_plan("/Game/Maps/Terrain_016_OLY/Terrain_016")
        .unwrap()
        .unwrap();
    assert_eq!(plan.components.len(), 16);
    assert_eq!(
        (
            plan.stat_container,
            plan.actor_state,
            plan.inventory_component
        ),
        (0, 4, 1)
    );
    assert_eq!(plan.components[0].name, "Stat Container");
    assert_eq!(plan.components[4].name, "ActorState");
    assert_eq!(plan.components[1].name, "InventoryComponent");
    // The level went out first and the client acked it (F-796).
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    cache.export_static(&mut exports, &plan.level).unwrap();
    exports.commit_bunch(1);
    exports.received_ack(1);
    let state = PawnOpenState {
        key: ActorKey::Session(0),
        controller: 2,
        player_state: 4,
        location: [0.0, 0.0, 0.0],
        content: PawnContent::default(),
    };
    let open = compose_pawn_open(
        &mut cache,
        &mut exports,
        &plan,
        &state,
        5,
        ByteOrder::Little,
    )
    .unwrap();
    let codec = ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    // The prefix carries the archetype, then the 16 components.
    let mut map = PackageMapState::new();
    let decoded = map
        .consume_export_prefix(&mut open.exports.unwrap().reader(), &codec)
        .unwrap();
    assert_eq!(decoded.len(), 17);
    assert_eq!(
        map.resolve_full_path(decoded[0].guid).as_deref(),
        Some(
            "/Game/BP/Player/BP_IcarusPlayerCharacterSurvival.Default__BP_IcarusPlayerCharacterSurvival_C"
        )
    );
    let mut component_paths: Vec<&str> = decoded[1..]
        .iter()
        .map(|reference| map.resolve_path(reference.guid).unwrap())
        .collect();
    let mut names: Vec<&str> = plan
        .components
        .iter()
        .map(|component| component.name.as_str())
        .collect();
    component_paths.sort_unstable();
    names.sort_unstable();
    assert_eq!(component_paths, names);
}

#[test]
fn builtin_pawn_content_composes_every_block() {
    use icarus_bevy::experimental::{
        actor_channel::{
            ActorChannelCodec, ActorChannelLimits, BlockTarget, PackageMapState, SubObjectClass,
            read_actor_bunch_body,
        },
        archive::ByteOrder,
        pawn_open::{
            CharacterVitals, EmptySlot, FastArrayContent, INVENTORY_SLOTS_FIELD_INDEX, PawnContent,
            PawnInventory, PawnOpenState, STAT_ARRAY_FIELD_INDEX, compose_pawn_open,
        },
        rep_layout::{
            GameplayTag, RepLayoutCodec, RepLayoutLimits, RepValue, StatPairRepState,
            actor_component_rep_layout, inventory_rep_layout, player_character_state_rep_layout,
            read_rep_properties,
        },
        rep_rpc::{
            FAST_ARRAY_COUNT_CAP, FastArrayItem, FastArrayItemLayout, read_custom_delta_header,
            read_fast_array_delta, read_field_header,
        },
        session_open::{ActorKey, ConnectionExports, NetGuidCache},
    };
    let profile = ReleaseProfile::release_3_0_32();
    let plan = profile
        .pawn_open_plan("/Game/Maps/Terrain_016_OLY/Terrain_016")
        .unwrap()
        .unwrap();
    // Authored values in the captured shapes (F-797), not release data.
    let vitals = CharacterVitals {
        health: 90,
        max_health: 120,
        stamina: 40,
        max_stamina: 50,
        oxygen: 7,
        max_oxygen: 8,
        water: 9,
        max_water: 10,
        food: 11,
        max_food: 12,
        biome_row: 3,
        growth_row: 4,
    };
    let content = PawnContent {
        icarus_uid: Some(4_242),
        gameplay_tags: vec![5, 1_000],
        frozen_movement: true,
        character_state: vitals.values(),
        stats: Some(FastArrayContent {
            replication_key: 3,
            items: vec![
                StatPairRepState { stat: 1, value: 25 },
                StatPairRepState {
                    stat: 9,
                    value: -400,
                },
            ],
        }),
        inventories: vec![
            PawnInventory {
                info_row: 6,
                slots: FastArrayContent {
                    replication_key: 2,
                    items: (0..3)
                        .map(|index| EmptySlot {
                            query_row: 30 + index as u16,
                            index,
                        })
                        .collect(),
                },
            },
            PawnInventory {
                info_row: 7,
                slots: FastArrayContent::default(),
            },
        ],
    };
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let state = PawnOpenState {
        key: ActorKey::Session(0),
        controller: 2,
        player_state: 4,
        location: [1.0, 2.0, 3.0],
        content: content.clone(),
    };
    let open = compose_pawn_open(
        &mut cache,
        &mut exports,
        &plan,
        &state,
        3,
        ByteOrder::Little,
    )
    .unwrap();
    let codec = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    // The inventory class is exported once, after the components.
    let mut map = PackageMapState::new();
    let decoded = map
        .consume_export_prefix(&mut open.exports.unwrap().reader(), &codec)
        .unwrap();
    assert_eq!(
        map.resolve_full_path(decoded.last().unwrap().guid)
            .as_deref(),
        Some("/Script/Icarus.Inventory")
    );
    let body = read_actor_bunch_body(&mut open.bunch.payload.reader(), true, &codec).unwrap();
    assert_eq!(body.blocks.len(), 1 + 16 + 2);
    let actor = read_rep_properties(
        &mut body.blocks[0].payload.reader(),
        &plan.layout,
        &rep_codec,
    )
    .unwrap();
    let value = |handle: u32| {
        actor
            .entries
            .iter()
            .find(|entry| entry.handle == handle)
            .map(|entry| entry.value.clone())
    };
    assert_eq!(value(46), Some(RepValue::Int(4_242)));
    assert_eq!(
        value(50),
        Some(RepValue::GameplayTagContainer(Some(vec![
            GameplayTag::NetIndex(5),
            GameplayTag::NetIndex(1_000)
        ])))
    );
    assert_eq!(value(67), Some(RepValue::Bool(true)));
    let Some(RepValue::Object(inventory_component)) = value(56) else {
        panic!("InventoryComponent");
    };
    // Component blocks: the eight active ones carry only `bIsActive`, the
    // state its values, the stat container its one field, the rest nothing.
    let components = &body.blocks[1..17];
    for (index, (component, block)) in plan.components.iter().zip(components).enumerate() {
        if index == plan.stat_container {
            assert!(!block.has_rep_layout);
            let mut reader = block.payload.reader();
            let field =
                read_field_header(&mut reader, plan.content.stat_container_bound, 4096).unwrap();
            assert!(reader.at_end());
            assert_eq!(field.index, STAT_ARRAY_FIELD_INDEX);
            let mut field_reader = field.payload.reader();
            let header =
                read_custom_delta_header(&mut field_reader, profile.engine_network_version, 1)
                    .unwrap();
            assert_eq!(header.struct_delta, Some(true));
            let delta = read_fast_array_delta(
                &mut field_reader,
                &FastArrayItemLayout::Delta(vec![
                    icarus_bevy::experimental::rep_layout::RepCommand::StatPairRepState,
                ]),
                &rep_codec,
                FAST_ARRAY_COUNT_CAP,
            )
            .unwrap();
            assert!(field_reader.at_end());
            assert_eq!(
                (delta.array_replication_key, delta.base_replication_key),
                (3, 0)
            );
            let ids: Vec<i32> = delta.changed.iter().map(|(id, _)| *id).collect();
            assert_eq!(ids, [1, 2]);
            let FastArrayItem::Delta(second) = &delta.changed[1].1 else {
                panic!("delta item");
            };
            assert_eq!(
                second.entries[0].value,
                RepValue::StatPairRepState(StatPairRepState {
                    stat: 9,
                    value: -400
                })
            );
        } else if index == plan.actor_state {
            assert!(block.has_rep_layout);
            let state = read_rep_properties(
                &mut block.payload.reader(),
                &player_character_state_rep_layout(),
                &rep_codec,
            )
            .unwrap();
            let decoded: std::collections::BTreeMap<u32, RepValue> = state
                .entries
                .into_iter()
                .map(|entry| (entry.handle, entry.value))
                .collect();
            assert_eq!(decoded, vitals.values());
        } else if component.rep_bools.is_empty() {
            assert!(!block.has_rep_layout && block.payload.bit_len() == 0);
        } else {
            let bools = read_rep_properties(
                &mut block.payload.reader(),
                &actor_component_rep_layout(),
                &rep_codec,
            )
            .unwrap();
            assert_eq!(bools.entries.len(), 1);
            assert_eq!(bools.entries[0].value, RepValue::Bool(true));
        }
    }
    assert_eq!(
        plan.components
            .iter()
            .filter(|component| !component.rep_bools.is_empty())
            .count(),
        8
    );
    // Inventories: class-named, owned by the InventoryComponent, then slots.
    for (block, inventory) in body.blocks[17..].iter().zip(&content.inventories) {
        let BlockTarget::SubObject {
            class: SubObjectClass::Class(_),
            ..
        } = &block.target
        else {
            panic!("a runtime subobject");
        };
        let mut reader = block.payload.reader();
        let properties =
            read_rep_properties(&mut reader, &inventory_rep_layout(), &rep_codec).unwrap();
        let handles: Vec<(u32, RepValue)> = properties
            .entries
            .into_iter()
            .map(|entry| (entry.handle, entry.value))
            .collect();
        assert_eq!(handles[0], (2, RepValue::Bool(true)));
        assert_eq!(
            handles[1],
            (3, RepValue::Object(inventory_component.clone()))
        );
        assert_eq!(handles[2], (7, RepValue::Byte(inventory.info_row)));
        let field = read_field_header(&mut reader, plan.content.inventory_bound, 1 << 16).unwrap();
        assert!(reader.at_end());
        assert_eq!(field.index, INVENTORY_SLOTS_FIELD_INDEX);
        let mut field_reader = field.payload.reader();
        read_custom_delta_header(&mut field_reader, profile.engine_network_version, 1).unwrap();
        let delta = read_fast_array_delta(
            &mut field_reader,
            &FastArrayItemLayout::Delta(plan.content.slot_layout.clone()),
            &rep_codec,
            FAST_ARRAY_COUNT_CAP,
        )
        .unwrap();
        assert!(field_reader.at_end());
        assert_eq!(delta.changed.len(), inventory.slots.items.len());
        for ((_, item), slot) in delta.changed.iter().zip(&inventory.slots.items) {
            let FastArrayItem::Delta(stream) = item else {
                panic!("delta item");
            };
            let slot_handles: Vec<u32> = stream.entries.iter().map(|entry| entry.handle).collect();
            assert_eq!(slot_handles, [2, 10, 11, 12, 13, 14, 15, 16, 17, 18]);
            assert_eq!(stream.entries[5].value, RepValue::UInt16(slot.query_row));
            assert_eq!(stream.entries[9].value, RepValue::Int(slot.index));
        }
    }
    // Character state handles outside 3..=32 fail closed.
    let mut bad = state;
    bad.content.character_state.insert(2, RepValue::Bool(true));
    assert!(
        compose_pawn_open(
            &mut NetGuidCache::new(),
            &mut ConnectionExports::new(),
            &plan,
            &bad,
            3,
            ByteOrder::Little,
        )
        .is_err()
    );
}

#[test]
fn pawn_sections_parse_and_fail_closed() {
    let release = "[release]\napp_id = 2089300\nbuild_id = 1\nproject_version = \"1.0\"\n";
    let release = release.replace(
        "\"1.0\"\n",
        &format!(
            "\"1.0\"\nnetwork_checksum = {}\n",
            icarus_bevy::experimental::release::network_checksum("1.0")
        ),
    );
    let tags = "[tags]\ndata_table_tag_count = 130\nini_tag_count = 1889\nunique_node_count = 2302\ntotal_bits = 12\nconfigured_first_segment = 16\neffective_first_segment = 0\ncontainer_bits = 6\nnet_index_hash = 1829726063\n";
    let components = "components = [{ name = \"S\", checksum = 2 }, { name = \"I\", checksum = 3, rep_bools = [[2, true]] }, { name = \"A\", checksum = 4 }]\n";
    let section = format!(
        "[pawn]\n\
        archetype = [{{ path = \"Default__P_C\", checksum = 1 }}, {{ path = \"/Game/P\", checksum = 0 }}]\n\
        {components}\
        stat_container = \"S\"\n\
        actor_state = \"A\"\n\
        inventory_component = \"I\"\n\
        inventory_class = [{{ path = \"Bag\", checksum = 9 }}, {{ path = \"/Script/P\", checksum = 0 }}]\n"
    );
    let base = format!("{release}{tags}");
    // Unknown builds get no built-in identifiers.
    let without = ReleaseProfile::parse(&base).unwrap();
    assert!(without.pawn.is_none());
    assert!(
        without
            .pawn_open_plan("/Game/Maps/Test/Test")
            .unwrap()
            .is_none()
    );
    let parsed = ReleaseProfile::parse(&format!("{base}{section}")).unwrap();
    assert_ne!(parsed.pawn, ReleaseProfile::release_3_0_32().pawn);
    // The section round-trips through the plan.
    let pawn = parsed.pawn.as_ref().unwrap();
    assert_eq!(pawn.archetype[0].path, "Default__P_C");
    assert_eq!(pawn.components.len(), 3);
    let plan = parsed
        .pawn_open_plan("/Game/Maps/Test/Test")
        .unwrap()
        .unwrap();
    assert_eq!(
        (
            plan.stat_container,
            plan.actor_state,
            plan.inventory_component
        ),
        (0, 2, 1)
    );
    assert_eq!(plan.components[2].name, "A");
    assert_eq!(plan.components[2].checksum, 4);
    assert_eq!(plan.archetype[1].path, "/Game/P");
    assert_eq!(plan.components[1].rep_bools, [(2, true)]);
    assert!(plan.components[0].rep_bools.is_empty());
    assert_eq!(plan.content.inventory_class[0].path, "Bag");
    for bad in [
        section.replace("rep_bools = [[2, true]]", "rep_bools = [[3, true]]"),
        section.replace(
            "rep_bools = [[2, true]]",
            "rep_bools = [[2, true], [1, false]]",
        ),
        section.replace(
            "{ path = \"Bag\", checksum = 9 }",
            "{ path = \"Bag\", checksum = 0 }",
        ),
        section.replace(", { path = \"/Script/P\", checksum = 0 }", ""),
        section
            .lines()
            .filter(|line| !line.starts_with("inventory_class"))
            .map(|line| format!("{line}\n"))
            .collect(),
        section.replace("checksum = 3", "checksum = 0"),
        section.replace(
            "checksum = 4 }]",
            "checksum = 4 }, { name = \"a\", checksum = 5 }]",
        ),
        section.replace("name = \"A\"", "name = \"\""),
        section.replace("actor_state = \"A\"", "actor_state = \"X\""),
        section.replace("actor_state = \"A\"", "actor_state = \"S\""),
        section.replace(", { path = \"/Game/P\", checksum = 0 }", ""),
        section.replace("checksum = 4 }", "checksum = 4, extra = 1 }"),
        format!("{section}extra = 1\n"),
        section.replace(components, "components = []\n"),
    ] {
        assert_ne!(bad, section);
        assert!(
            ReleaseProfile::parse(&format!("{base}{bad}")).is_err(),
            "{bad}"
        );
    }
}

/// World manager identifiers (F-859): built in for the qualified builds,
/// none for unknown ones, parsed from `[managers]` and validated.
#[test]
fn manager_sections_parse_and_fail_closed() {
    use icarus_bevy::experimental::manager_open::ManagerSlot;
    let builtin = ReleaseProfile::release_3_0_32();
    let plans = builtin.manager_open_plans("/Game/Maps/Test/Test").unwrap();
    let slots: Vec<ManagerSlot> = plans.iter().map(|(slot, _)| *slot).collect();
    assert_eq!(
        slots,
        [
            ManagerSlot::WorldTalent,
            ManagerSlot::Radiation,
            ManagerSlot::Disaster
        ]
    );
    assert_eq!(plans[1].1.archetype[0].path, "Default__RadiationManager");
    assert_eq!(plans[1].1.archetype[1].path, "/Script/Icarus");
    assert_eq!(plans[1].1.components.len(), 2);
    assert_eq!(plans[2].1.components.len(), 1);
    assert_eq!(plans[2].1.level[2].path, "/Game/Maps/Test/Test");
    let release = "[release]\napp_id = 2089300\nbuild_id = 1\nproject_version = \"1.0\"\n";
    let release = release.replace(
        "\"1.0\"\n",
        &format!(
            "\"1.0\"\nnetwork_checksum = {}\n",
            icarus_bevy::experimental::release::network_checksum("1.0")
        ),
    );
    let tags = "[tags]\ndata_table_tag_count = 130\nini_tag_count = 1889\nunique_node_count = 2302\ntotal_bits = 12\nconfigured_first_segment = 16\neffective_first_segment = 0\ncontainer_bits = 6\nnet_index_hash = 1829726063\n";
    let base = format!("{release}{tags}");
    let unknown = ReleaseProfile::parse(&base).unwrap();
    assert!(unknown.managers.is_none());
    assert!(
        unknown
            .manager_open_plans("/Game/Maps/Test/Test")
            .unwrap()
            .is_empty()
    );
    let section = "[managers]\n\
        radiation_manager = [{ path = \"Default__R\", checksum = 1 }, { path = \"/Script/I\", checksum = 0 }]\n\
        world_talent_manager = [{ path = \"Default__W\", checksum = 2 }, { path = \"/Script/I\", checksum = 0 }]\n\
        disaster_controller = [{ path = \"Default__D_C\", checksum = 3 }, { path = \"/Game/D\", checksum = 0 }]\n\
        stat_container_checksum = 4\n\
        actor_state_checksum = 5\n";
    let parsed = ReleaseProfile::parse(&format!("{base}{section}")).unwrap();
    let plans = parsed.manager_open_plans("/Game/Maps/Test/Test").unwrap();
    assert_eq!(plans[0].1.archetype[0].path, "Default__W");
    assert_eq!(plans[0].1.components[0].checksum, 4);
    assert_eq!(plans[0].1.components[1].checksum, 5);
    for bad in [
        section.replace("checksum = 1 }", "checksum = 0 }"),
        section.replace(", { path = \"/Game/D\", checksum = 0 }", ""),
        section.replace("actor_state_checksum = 5\n", ""),
        format!("{section}extra = 1\n"),
    ] {
        assert_ne!(bad, section);
        assert!(
            ReleaseProfile::parse(&format!("{base}{bad}")).is_err(),
            "{bad}"
        );
    }
}
