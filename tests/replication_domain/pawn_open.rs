use icarus_bevy::experimental::{
    actor_channel::{ActorChannelCodec, ActorChannelLimits, BlockTarget, read_actor_bunch_body},
    archive::ByteOrder,
    pawn_open::{PawnContent, PawnOpenState, compose_pawn_open},
    release::ReleaseProfile,
    rep_layout::{
        PawnLayoutVersion, RepCommand, RepLayoutCodec, RepLayoutLimits, RepValue,
        read_rep_properties,
    },
    session_open::{ActorKey, ConnectionExports, NetGuidCache},
};

#[test]
fn reworked_pawn_layout_inserts_one_boolean_after_is_male() {
    let map = "/Game/Maps/Test/Test";
    let legacy = ReleaseProfile::fixture("3.0.32")
        .pawn_open_plan(map)
        .unwrap()
        .unwrap();
    let updated = ReleaseProfile::fixture("3.0.34")
        .pawn_open_plan(map)
        .unwrap()
        .unwrap();
    assert_eq!(legacy.layout.len(), 89);
    assert_eq!(updated.layout.len(), 90);
    assert_eq!(&legacy.layout[..59], &updated.layout[..59]);
    assert_eq!(updated.layout[59], RepCommand::Bool);
    assert_eq!(&legacy.layout[59..], &updated.layout[60..]);
    assert_eq!(legacy.content, updated.content);
    for version in [
        PawnLayoutVersion::Legacy,
        PawnLayoutVersion::ReworkedCharacter,
    ] {
        for invalid in [0, 90, u32::MAX] {
            assert!(version.wire_handle(invalid).is_err());
        }
        assert_eq!(version.wire_handle(59).unwrap(), 59);
    }
    assert_eq!(PawnLayoutVersion::Legacy.wire_handle(60).unwrap(), 60);
    assert_eq!(
        PawnLayoutVersion::ReworkedCharacter
            .wire_handle(60)
            .unwrap(),
        61
    );
    assert_eq!(
        PawnLayoutVersion::ReworkedCharacter
            .wire_handle(89)
            .unwrap(),
        90
    );
}

#[test]
fn pawn_open_writes_frozen_movement_at_the_selected_releases_wire_handle() {
    for (profile, frozen_handle) in [
        (ReleaseProfile::fixture("3.0.31"), 67),
        (ReleaseProfile::fixture("3.0.32"), 67),
        (ReleaseProfile::fixture("3.0.34"), 68),
    ] {
        let plan = profile
            .pawn_open_plan("/Game/Maps/Test/Test")
            .unwrap()
            .unwrap();
        let mut cache = NetGuidCache::new();
        let mut exports = ConnectionExports::new();
        let state = PawnOpenState {
            key: ActorKey::Session(1),
            controller: 2,
            player_state: 4,
            location: [0.0; 3],
            content: PawnContent {
                frozen_movement: true,
                ..PawnContent::default()
            },
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
        let body = read_actor_bunch_body(&mut open.bunch.payload.reader(), true, &codec).unwrap();
        let block = body
            .blocks
            .iter()
            .find(|block| block.target == BlockTarget::Actor)
            .unwrap();
        let mut reader = block.payload.reader();
        let stream = read_rep_properties(
            &mut reader,
            &plan.layout,
            &RepLayoutCodec {
                actor: codec,
                checksums: false,
                limits: RepLayoutLimits::default(),
            },
        )
        .unwrap();
        assert!(reader.at_end());
        let tail: Vec<_> = stream
            .entries
            .iter()
            .filter(|entry| entry.handle >= 59)
            .collect();
        assert_eq!(tail.len(), 1);
        assert_eq!(tail[0].handle, frozen_handle);
        assert_eq!(tail[0].value, RepValue::Bool(true));
    }
}

/// The frozen-movement release is one actor property block, no spawn, that
/// carries only `bServerFrozenMovement` at the release's wire handle: 67 in
/// the legacy layout, 68 from 3.0.34 (the 3.0.35 pin inherits that layout).
#[test]
fn frozen_movement_update_writes_only_the_flag_at_the_selected_releases_wire_handle() {
    use icarus_bevy::experimental::pawn_open::compose_pawn_frozen_movement_update;
    for (profile, frozen_handle) in [
        (ReleaseProfile::fixture("3.0.31"), 67),
        (ReleaseProfile::fixture("3.0.32"), 67),
        (ReleaseProfile::fixture("3.0.34"), 68),
        (ReleaseProfile::fixture("3.0.35"), 68),
    ] {
        let plan = profile
            .pawn_open_plan("/Game/Maps/Test/Test")
            .unwrap()
            .unwrap();
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let codec = ActorChannelCodec {
                engine_net_version: profile.engine_network_version,
                exports_enabled: false,
                has_level: true,
                order,
                limits: ActorChannelLimits::default(),
            };
            for frozen in [false, true] {
                let payload = compose_pawn_frozen_movement_update(&plan, frozen, order).unwrap();
                let body = read_actor_bunch_body(&mut payload.reader(), false, &codec).unwrap();
                assert!(body.spawn.is_none());
                let [block] = body.blocks.as_slice() else {
                    panic!("one block");
                };
                assert_eq!(block.target, BlockTarget::Actor);
                assert!(block.has_rep_layout);
                // The handle, one bit and the terminator: 18 bits for the
                // 3.0.34/35 handle 68, as the original's own update measures.
                if frozen_handle == 68 {
                    assert_eq!(block.payload.bit_len(), 18);
                }
                let mut reader = block.payload.reader();
                let stream = read_rep_properties(
                    &mut reader,
                    &plan.layout,
                    &RepLayoutCodec {
                        actor: codec,
                        checksums: false,
                        limits: RepLayoutLimits::default(),
                    },
                )
                .unwrap();
                assert!(reader.at_end());
                let [entry] = stream.entries.as_slice() else {
                    panic!("one property");
                };
                assert_eq!(entry.handle, frozen_handle);
                assert_eq!(entry.value, RepValue::Bool(frozen));
            }
        }
    }
}
