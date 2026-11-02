use icarus_bevy::experimental::actor_channel::{
    ActorChannelCodec, ActorChannelLimits, GuidKind, PackageMapState,
};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::session_open::{
    ComponentBlock, ComponentExport, ConnectionExports, NetGuidCache, PathSegment,
    PlayerControllerProfile, encode_exports,
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

#[test]
fn authored_actor_flags_preserve_explicit_false_sorted_handles_and_legacy_bytes() {
    use icarus_bevy::{
        experimental::{
            rep_layout::{
                MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
                actor_rep_layout, parse_rep_properties,
            },
            session_open::{
                actor_properties_update_block, authored_movement, movement_update_block,
                world_actor_block, world_actor_state_block,
            },
        },
        replication::NetActorFlags,
    };
    let movement = authored_movement([1.0; 3], [90.0; 3], [2.0; 3]).unwrap();
    for motion in [None, Some(movement.clone())] {
        assert_eq!(
            world_actor_state_block(17, motion.clone(), None).unwrap(),
            world_actor_block(17, motion.clone()).unwrap()
        );
        for hidden in [false, true] {
            for can_be_damaged in [false, true] {
                let flags = NetActorFlags {
                    hidden,
                    can_be_damaged,
                };
                let block = world_actor_state_block(17, motion.clone(), Some(flags)).unwrap();
                for order in [ByteOrder::Little, ByteOrder::Big] {
                    let rep = RepLayoutCodec {
                        actor: ActorChannelCodec { order, ..codec() },
                        checksums: false,
                        limits: RepLayoutLimits::default(),
                    };
                    let layout = actor_rep_layout(
                        MovementLevel::Whole,
                        MovementLevel::Whole,
                        RotationLevel::Bytes,
                    );
                    let stream = parse_rep_properties(&block.payload, &layout, &rep).unwrap();
                    let handles: Vec<_> = stream.entries.iter().map(|e| e.handle).collect();
                    assert_eq!(
                        handles,
                        if motion.is_some() {
                            vec![1, 4, 5, 6, 14]
                        } else {
                            vec![1, 4, 5, 14]
                        }
                    );
                    assert_eq!(stream.entries[0].value, RepValue::Bool(hidden));
                    assert_eq!(stream.entries[1].value, RepValue::Bool(can_be_damaged));
                    let update = actor_properties_update_block(
                        17,
                        motion.clone(),
                        Some(hidden),
                        Some(can_be_damaged),
                    )
                    .unwrap();
                    let decoded = parse_rep_properties(&update.payload, &layout, &rep).unwrap();
                    assert_eq!(decoded.entries[0].value, RepValue::Bool(hidden));
                    assert_eq!(decoded.entries[1].value, RepValue::Bool(can_be_damaged));
                    assert!(
                        decoded
                            .entries
                            .iter()
                            .all(|e| matches!(e.handle, 1 | 4 | 6))
                    );
                }
            }
        }
    }
    assert_eq!(
        movement_update_block(17, movement.clone()).unwrap(),
        actor_properties_update_block(17, Some(movement), None, None).unwrap()
    );
}

#[test]
fn authored_spawn_motion_and_property_six_keep_their_independent_precision() {
    use icarus_bevy::experimental::{
        actor_channel::read_actor_bunch_body,
        rep_layout::{
            MovementLevel, MovementRotation, RepLayoutCodec, RepLayoutLimits, RepValue,
            RotationLevel, actor_rep_layout, read_rep_properties,
        },
        session_open::{ActorOpenSpec, authored_movement, compose_actor_open, world_actor_block},
    };
    let location = [12.26, -34.26, 0.0];
    let rotation = [1.0, -90.0, 360.0];
    let velocity = [5.26, -8.26, 0.0];
    let movement = authored_movement(location, rotation, velocity).unwrap();
    let spec = ActorOpenSpec {
        actor: 2,
        archetype: profile().archetype,
        level: profile().level,
        location: Some(location),
        rotation: Some(rotation),
        velocity: Some(velocity),
        blocks: vec![world_actor_block(17, Some(movement.clone())).unwrap()],
    };
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let composed = compose_actor_open(
            &mut NetGuidCache::new(),
            &mut ConnectionExports::new(),
            &spec,
            2,
            17,
            order,
        )
        .unwrap();
        let actor = ActorChannelCodec { order, ..codec() };
        let body =
            read_actor_bunch_body(&mut composed.bunch.payload.reader(), true, &actor).unwrap();
        let spawn = body.spawn.unwrap().dynamic.unwrap();
        let close = |actual: [f32; 3], expected: [f32; 3]| {
            for (actual, expected) in actual.into_iter().zip(expected) {
                assert!((actual - expected).abs() < 0.0001);
            }
        };
        close(spawn.location.unwrap().decode(), [12.3, -34.3, 0.0]);
        close(spawn.velocity.unwrap().decode(), [5.3, -8.3, 0.0]);
        let spawn_rotation = spawn.rotation.unwrap().degrees();
        assert!((spawn_rotation[0] - 1.0).abs() < 360.0 / 65536.0);
        assert_eq!(spawn_rotation[1..], [270.0, 0.0]);
        let rep = RepLayoutCodec {
            actor,
            checksums: false,
            limits: RepLayoutLimits::default(),
        };
        let stream = read_rep_properties(
            &mut body.blocks[0].payload.reader(),
            &actor_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
            ),
            &rep,
        )
        .unwrap();
        let entry = stream
            .entries
            .iter()
            .find(|entry| entry.handle == 6)
            .unwrap();
        let RepValue::Movement(decoded) = &entry.value else {
            panic!("movement")
        };
        assert_eq!(decoded, &movement);
        assert_eq!(decoded.location.unbiased(), [12, -34, 0]);
        assert_eq!(decoded.linear_velocity.unbiased(), [5, -8, 0]);
        let Some(MovementRotation::Bytes(rotation)) = decoded.rotation else {
            panic!("byte rotator")
        };
        assert_eq!(rotation.degrees(), [1.40625, 270.0, 0.0]);
    }
}

#[test]
fn invalid_spawn_motion_rejects_before_guid_or_export_changes_and_can_retry() {
    use icarus_bevy::experimental::session_open::{
        ActorOpenSpec, authored_movement, compose_actor_open, world_actor_state_block,
    };
    // Both values fit property-6 defaults but exceed the independent spawn
    // representations. Valid property data must not bypass spawn validation.
    assert!(authored_movement([0.0; 3], [1.0e17, 0.0, 0.0], [2_000_000.0, 0.0, 0.0]).is_ok());
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let valid = ActorOpenSpec {
            actor: 2,
            archetype: profile().archetype,
            level: profile().level,
            location: Some([0.0; 3]),
            rotation: Some([0.0; 3]),
            velocity: Some([0.0; 3]),
            blocks: vec![
                world_actor_state_block(
                    17,
                    Some(authored_movement([0.0; 3], [0.0; 3], [0.0; 3]).unwrap()),
                    Some(icarus_bevy::replication::NetActorFlags {
                        hidden: true,
                        can_be_damaged: true,
                    }),
                )
                .unwrap(),
            ],
        };
        let mut cache = NetGuidCache::new();
        let mut exports = ConnectionExports::new();
        for field in 0..3 {
            // Velocity fits property-6 whole packing but not spawn <10,24>.
            for bad in [
                f32::NAN,
                f32::INFINITY,
                -f32::INFINITY,
                f32::MAX,
                1.0e17,
                2_000_000.0,
            ] {
                if field == 1 && bad == 2_000_000.0 {
                    continue;
                }
                let mut spec = valid.clone();
                match field {
                    0 => spec.location = Some([bad, 0.0, 0.0]),
                    1 => spec.rotation = Some([bad, 0.0, 0.0]),
                    _ => spec.velocity = Some([bad, 0.0, 0.0]),
                }
                let before = (format!("{cache:?}"), format!("{exports:?}"));
                assert!(compose_actor_open(&mut cache, &mut exports, &spec, 2, 17, order).is_err());
                assert_eq!((format!("{cache:?}"), format!("{exports:?}")), before);
            }
        }
        let composed = compose_actor_open(&mut cache, &mut exports, &valid, 2, 17, order).unwrap();
        let fresh = compose_actor_open(
            &mut NetGuidCache::new(),
            &mut ConnectionExports::new(),
            &valid,
            2,
            17,
            order,
        )
        .unwrap();
        assert_eq!(composed.bunch.payload, fresh.bunch.payload);
        assert_eq!(composed.exports, fresh.exports);
        let mut absent = valid;
        absent.rotation = None;
        absent.velocity = None;
        let composed = compose_actor_open(&mut cache, &mut exports, &absent, 3, 17, order).unwrap();
        let body = icarus_bevy::experimental::actor_channel::read_actor_bunch_body(
            &mut composed.bunch.payload.reader(),
            true,
            &ActorChannelCodec { order, ..codec() },
        )
        .unwrap();
        let spawn = body.spawn.unwrap().dynamic.unwrap();
        assert!(spawn.rotation.is_none() && spawn.velocity.is_none());
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
    let (guids, references) = cache
        .player_controller_exports(&mut ConnectionExports::new(), &profile)
        .unwrap();
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
    let (_, references) = cache
        .player_controller_exports(&mut ConnectionExports::new(), &profile)
        .unwrap();
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
fn a_path_is_exported_once_per_bunch_until_a_carrying_packet_is_acked() {
    let profile = profile();
    let mut cache = NetGuidCache::new();
    cache.register_level(&profile.level);
    let mut first_connection = ConnectionExports::new();
    let first = cache
        .export_static(&mut first_connection, &profile.archetype)
        .unwrap();
    // Once per bunch (`ExportNetGUIDs`).
    let again = cache
        .export_static(&mut first_connection, &profile.archetype)
        .unwrap();
    assert!(first.export.is_some());
    assert_eq!(again.guid, first.guid);
    assert!(again.export.is_none());
    let exported: Vec<u32> = first_connection.bunch_exports().collect();
    assert_eq!(exported.len(), 2, "the archetype and its package");
    // Committed to packet 10 but not acked: the next bunch exports it again.
    first_connection.commit_bunch(10);
    first_connection.begin_bunch();
    let pending = cache
        .export_static(&mut first_connection, &profile.archetype)
        .unwrap();
    assert_eq!(pending, first);
    // A second commit keeps the first packet; a nak of the later one changes
    // nothing, a nak of packet 10 returns the GUID to not acked.
    first_connection.commit_bunch(11);
    first_connection.received_nak(11);
    first_connection.received_ack(9);
    assert!(!first_connection.is_acked(first.guid));
    first_connection.received_nak(10);
    first_connection.begin_bunch();
    cache
        .export_static(&mut first_connection, &profile.archetype)
        .unwrap();
    first_connection.commit_bunch(12);
    // An ack of a later packet acks it; then only plain references follow.
    first_connection.received_ack(13);
    assert!(first_connection.is_acked(first.guid));
    let acked = cache
        .export_static(&mut first_connection, &profile.archetype)
        .unwrap();
    assert!(acked.export.is_none());
    // A second connection shares the GUID but still receives the full export.
    let mut second_connection = ConnectionExports::new();
    let other = cache
        .export_static(&mut second_connection, &profile.archetype)
        .unwrap();
    assert_eq!(other, first);
    assert!(!second_connection.is_acked(first.guid));
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
    // |v * 10| must stay below 1 << 24; 4.3e8 must fail rather than wrap.
    assert!(quantize_spawn_vector([1_600_000.0, 0.0, 0.0]).is_ok());
    assert!(quantize_spawn_vector([1_700_000.0, 0.0, 0.0]).is_err());
    assert!(quantize_spawn_vector([430_000_000.0, 0.0, 0.0]).is_err());
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
    let (guids, _) = cache
        .player_controller_exports(&mut ConnectionExports::new(), &profile)
        .unwrap();
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
    // Mismatched component counts fail loudly instead of truncating.
    let mut short = guids.clone();
    short.components.pop();
    assert!(
        compose_player_controller_open_body(
            &short,
            &profile,
            &state,
            &layout,
            &actor_component_rep_layout(),
            &rep_codec,
        )
        .is_err()
    );
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

#[test]
fn player_state_open_reuses_exported_paths_and_carries_player_values() {
    use icarus_bevy::experimental::actor_channel::read_actor_bunch_body;
    use icarus_bevy::experimental::control_messages::{
        ArchiveText, NetIdType, NetIdValue, UniqueNetId,
    };
    use icarus_bevy::experimental::rep_layout::{
        MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
        bp_icarus_player_state_rep_layout, read_rep_properties,
    };
    use icarus_bevy::experimental::session_open::{
        PlayerStateOpenPlan, PlayerStateOpenState, compose_player_state_open,
        replicated_unique_net_id,
    };
    let profile = profile();
    let plan = PlayerStateOpenPlan {
        archetype: vec![
            PathSegment::new("Default__TestState_C", 0x0bad_cafe, false),
            PathSegment::new("/Game/Test/TestState", 0, false),
        ],
        level: profile.level.clone(),
        layout: bp_icarus_player_state_rep_layout(
            MovementLevel::Whole,
            MovementLevel::Whole,
            RotationLevel::Bytes,
        ),
        engine_net_version: 17,
        channel_index: 3,
    };
    let mut cache = NetGuidCache::new();
    cache.register_level(&profile.level);
    let mut exports = ConnectionExports::new();
    let (guids, _) = cache
        .player_controller_exports(&mut exports, &profile)
        .unwrap();
    // The controller's exports went out in packet 1, acked (F-796).
    exports.commit_bunch(1);
    exports.received_ack(1);
    let login = UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("null").unwrap()),
        value: NetIdValue::Text(ArchiveText::text("Player-0001").unwrap()),
    };
    let unique_id = replicated_unique_net_id(&login, &[(1, "NULL"), (3, "STEAM")]);
    assert_eq!(unique_id.kind, NetIdType::Indexed(1));
    assert_eq!(unique_id.value, login.value);
    let unknown = UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("EOS").unwrap()),
        value: login.value.clone(),
    };
    assert_eq!(replicated_unique_net_id(&unknown, &[(1, "NULL")]), unknown);
    let state = PlayerStateOpenState {
        player_state: guids.player_state,
        owner: guids.actor,
        player_id: 256,
        unique_id: unique_id.clone(),
        player_name: ArchiveText::text("Tester").unwrap(),
        is_host: true,
    };
    let open = compose_player_state_open(
        &mut cache,
        &mut exports,
        &plan,
        &state,
        icarus_bevy::experimental::archive::ByteOrder::Little,
    )
    .unwrap();
    // Only the new archetype chain is exported; the level is a plain reference.
    let mut map = icarus_bevy::experimental::actor_channel::PackageMapState::new();
    let decoded = map
        .consume_export_prefix(&mut open.exports.unwrap().reader(), &codec())
        .unwrap();
    assert_eq!(decoded.len(), 1);
    assert_eq!(
        map.resolve_full_path(decoded[0].guid).as_deref(),
        Some("/Game/Test/TestState.Default__TestState_C")
    );
    let header = &open.bunch.header;
    assert!(header.open && header.reliable && header.channel_index == 3);
    let body = read_actor_bunch_body(&mut open.bunch.payload.reader(), true, &codec()).unwrap();
    let spawn = body.spawn.unwrap();
    assert_eq!(spawn.actor.guid, guids.player_state);
    let dynamic = spawn.dynamic.unwrap();
    assert_eq!(dynamic.level.unwrap().guid, guids.level);
    assert!(dynamic.location.is_none() && dynamic.rotation.is_none());
    let rep_codec = RepLayoutCodec {
        actor: codec(),
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let stream = read_rep_properties(
        &mut body.blocks[0].payload.reader(),
        &plan.layout,
        &rep_codec,
    )
    .unwrap();
    let handles: Vec<u32> = stream.entries.iter().map(|entry| entry.handle).collect();
    assert_eq!(handles, [5, 13, 14, 17, 25, 26, 79]);
    let values: Vec<&RepValue> = stream.entries.iter().map(|entry| &entry.value).collect();
    assert!(matches!(values[1], RepValue::Object(owner) if owner.guid == guids.actor));
    assert!(matches!(values[3], RepValue::Int(256)));
    assert!(matches!(values[4], RepValue::UniqueNetId(id) if *id == unique_id));
    assert!(matches!(values[6], RepValue::Bool(true)));
    // Once its packet is acked, a second open to the same connection
    // exports nothing new.
    exports.commit_bunch(2);
    exports.received_ack(2);
    let again = compose_player_state_open(
        &mut cache,
        &mut exports,
        &plan,
        &state,
        icarus_bevy::experimental::archive::ByteOrder::Little,
    )
    .unwrap();
    assert!(again.exports.is_none());
}

/// A new static export whose outer the connection already holds names that
/// outer with only the bare checksum flag, as the original does (F-692,
/// F-859), and the export prefix round-trips.
#[test]
fn an_acked_outer_is_written_with_only_the_bare_checksum_flag() {
    use icarus_bevy::experimental::{
        actor_channel::{
            ActorChannelCodec, ActorChannelLimits, read_export_prefix, write_export_prefix,
        },
        archive::ByteOrder,
        packet::BitWriter,
        session_open::{ConnectionExports, NetGuidCache, PathSegment},
    };
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let package = PathSegment::new("/Script/Icarus", 0, false);
    let first = cache
        .export_static(&mut exports, std::slice::from_ref(&package))
        .unwrap();
    assert!(first.export.is_some());
    exports.commit_bunch(1);
    exports.received_ack(1);
    let archetype = cache
        .export_static(
            &mut exports,
            &[
                PathSegment::new("Default__RadiationManager", 2_158_376_861, false),
                package,
            ],
        )
        .unwrap();
    let export = archetype.export.as_ref().unwrap();
    let path = export.path.as_ref().unwrap();
    let outer = path.outer.export.as_ref().unwrap();
    assert_eq!(path.outer.guid, first.guid);
    assert!(outer.path.is_none() && outer.checksum.is_none() && outer.bare_checksum_flag);
    let codec = ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let mut writer = BitWriter::new();
    write_export_prefix(&mut writer, std::slice::from_ref(&archetype), &codec).unwrap();
    let payload = icarus_bevy::experimental::packet::Payload::from_writer(writer);
    let read = read_export_prefix(&mut payload.reader(), &codec).unwrap();
    assert_eq!(read, vec![archetype]);
}
