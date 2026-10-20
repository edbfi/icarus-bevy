use icarus_bevy::experimental::actor_channel::{
    ActorChannelCodec, ActorChannelLimits, GuidKind, PackageMapState,
};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::session_open::{
    ComponentBlock, ComponentExport, NetGuidCache, PathSegment, PlayerControllerProfile,
    encode_exports,
};

fn codec() -> ActorChannelCodec {
    ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    }
}

/// Authored identifiers in the shape of F-778 (not release data).
fn profile() -> PlayerControllerProfile {
    PlayerControllerProfile {
        archetype: vec![
            PathSegment::new("Default__TestController_C", 0x1234_5678, false),
            PathSegment::new("/Game/Test/TestController", 0, false),
        ],
        level: vec![
            PathSegment::new("PersistentLevel", 0, true),
            PathSegment::new("TestMap", 0, true),
            PathSegment::new("/Game/Maps/TestMap", 0, true),
        ],
        components: ["Alpha", "Beta", "Gamma"]
            .iter()
            .enumerate()
            .map(|(i, name)| ComponentExport {
                name: (*name).to_owned(),
                checksum: 100 + i as u32,
                block: ComponentBlock::None,
            })
            .collect(),
    }
}

#[test]
fn guid_numbering_follows_the_captured_policy() {
    let profile = profile();
    let mut cache = NetGuidCache::new();
    assert_eq!(cache.register_level(&profile.level), 3);
    let (guids, references) = cache.player_controller_exports(&profile).unwrap();
    assert_eq!(guids.actor, 2);
    assert_eq!(guids.level, 3);
    assert_eq!(guids.archetype, 9);
    assert_eq!(guids.player_state, 4);
    assert_eq!(guids.components, vec![6, 8, 10]);
    assert_eq!(
        references.iter().map(|r| r.guid).collect::<Vec<_>>(),
        vec![9, 3, 6, 8, 10]
    );
    // The archetype's outer is its package (11), whose outer is null.
    let archetype = references[0].summary();
    assert_eq!(archetype.path_units, Some(25));
    let package = archetype.outer.unwrap();
    assert!(package.export && package.checksum && !package.no_load);
    assert_eq!(package.outer.unwrap().kind, GuidKind::Null);
    // Components: no_load, checksummed, outer = the actor with bare flags only.
    let component = references[2].summary();
    assert!(component.no_load && component.checksum);
    let outer = component.outer.unwrap();
    assert_eq!(outer.kind, GuidKind::Dynamic);
    assert!(outer.export && outer.no_load && outer.checksum && outer.path_units.is_none());
}

#[test]
fn encoded_exports_decode_back_into_the_package_map() {
    let profile = profile();
    let mut cache = NetGuidCache::new();
    cache.register_level(&profile.level);
    let (_, references) = cache.player_controller_exports(&profile).unwrap();
    let payload = encode_exports(&references, &codec()).unwrap();
    let mut map = PackageMapState::new();
    let decoded = map
        .consume_export_prefix(&mut payload.reader(), &codec())
        .unwrap();
    assert_eq!(decoded, references);
    assert_eq!(
        map.resolve_full_path(9).as_deref(),
        Some("/Game/Test/TestController.Default__TestController_C")
    );
    assert_eq!(
        map.resolve_full_path(3).as_deref(),
        Some("/Game/Maps/TestMap.TestMap.PersistentLevel")
    );
    assert_eq!(map.get(6).unwrap().checksum, Some(100));
    assert_eq!(map.resolve_outer_guid(8), Some(2));
}

#[test]
fn a_connection_exports_each_static_path_once() {
    let profile = profile();
    let mut cache = NetGuidCache::new();
    cache.register_level(&profile.level);
    let first = cache.export_static(&profile.archetype).unwrap();
    let again = cache.export_static(&profile.archetype).unwrap();
    assert!(first.export.is_some());
    assert_eq!(again.guid, first.guid);
    assert!(again.export.is_none());
}

#[test]
fn spawn_vectors_quantize_like_write_packed_vector() {
    use icarus_bevy::experimental::actor_channel::SpawnVector;
    use icarus_bevy::experimental::session_open::quantize_spawn_vector;
    // max |v * 10| = 1034990 -> CeilLogTwo(1034991) = 20 -> 19 bits, bias 1 << 20.
    let SpawnVector::Quantized { bits, components } =
        quantize_spawn_vector([248.0, 3.0, 103_499.0]).unwrap()
    else {
        panic!("not quantized");
    };
    assert_eq!(bits, 19);
    assert_eq!(
        components,
        [2480 + (1 << 20), 30 + (1 << 20), 1_034_990 + (1 << 20)]
    );
    let SpawnVector::Quantized { bits, components } = quantize_spawn_vector([0.0; 3]).unwrap()
    else {
        panic!("not quantized");
    };
    assert_eq!((bits, components), (0, [2, 2, 2]));
    assert!(quantize_spawn_vector([f32::NAN, 0.0, 0.0]).is_err());
}

#[test]
fn open_body_carries_roles_references_and_component_blocks() {
    use icarus_bevy::experimental::actor_channel::{
        BlockTarget, encode_player_controller_open_bunch_body, parse_player_controller_open_bunch,
    };
    use icarus_bevy::experimental::packet::{Bunch, BunchHeader};
    use icarus_bevy::experimental::rep_layout::{
        MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel, TagEncoding,
        actor_component_rep_layout, icarus_player_controller_survival_rep_layout,
        read_rep_properties,
    };
    use icarus_bevy::experimental::session_open::{
        PlayerControllerOpenState, compose_player_controller_open_body,
    };
    let mut profile = profile();
    profile.components[0].block = ComponentBlock::RepBools(vec![(2, true)]);
    profile.components[2].block = ComponentBlock::CreateOnly;
    let mut cache = NetGuidCache::new();
    cache.register_level(&profile.level);
    let (guids, _) = cache.player_controller_exports(&profile).unwrap();
    let layout = icarus_player_controller_survival_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
        TagEncoding::NetIndex {
            first_segment: 12,
            total_bits: 12,
        },
    );
    let rep_codec = RepLayoutCodec {
        actor: codec(),
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let state = PlayerControllerOpenState {
        spawn_location: [100.0, -50.0, 2_000.0],
        net_player_index: 0,
        remote_role: 2,
        role: 3,
    };
    let body = compose_player_controller_open_body(
        &guids,
        &profile,
        &state,
        &layout,
        &actor_component_rep_layout(),
        &rep_codec,
    )
    .unwrap();
    // Actor block plus two component blocks (Beta has none).
    assert_eq!(body.blocks.len(), 3);
    let actor =
        read_rep_properties(&mut body.blocks[0].payload.reader(), &layout, &rep_codec).unwrap();
    let handles: Vec<u32> = actor.entries.iter().map(|e| e.handle).collect();
    assert_eq!(handles, vec![5, 14, 16, 19, 22, 23, 24]);
    assert!(matches!(actor.entries[0].value, RepValue::Byte(2)));
    assert!(matches!(actor.entries[1].value, RepValue::Byte(3)));
    assert!(matches!(&actor.entries[2].value, RepValue::Object(r) if r.guid == 4));
    let BlockTarget::SubObject { object, .. } = &body.blocks[1].target else {
        panic!("component block expected");
    };
    assert_eq!(object.guid, 6);
    assert!(!body.blocks[2].has_rep_layout && body.blocks[2].payload.bit_len() == 0);
    // The body survives the open-bunch encoder and parser unchanged.
    let payload = encode_player_controller_open_bunch_body(&body, &codec()).unwrap();
    let bunch = Bunch {
        header: BunchHeader {
            control: true,
            open: true,
            close: false,
            close_reason: None,
            paused: false,
            reliable: false,
            channel_index: 2,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: None,
            partial_initial: false,
            partial_final: false,
            channel_name: None,
        },
        payload,
    };
    assert_eq!(
        parse_player_controller_open_bunch(&bunch, &codec()).unwrap(),
        body
    );
}
