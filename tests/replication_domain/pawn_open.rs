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
    let legacy = ReleaseProfile::release_3_0_32()
        .pawn_open_plan(map)
        .unwrap()
        .unwrap();
    let updated = ReleaseProfile::release_3_0_34()
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
        (ReleaseProfile::release_3_0_31(), 67),
        (ReleaseProfile::release_3_0_32(), 67),
        (ReleaseProfile::release_3_0_34(), 68),
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
