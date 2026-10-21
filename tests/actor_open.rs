use std::collections::BTreeMap;

use icarus_bevy::experimental::actor_channel::{
    ActorChannelCodec, ActorChannelLimits, BlockTarget, PackageMapState, SubObjectClass,
    read_actor_bunch_body,
};
use icarus_bevy::experimental::actor_open::{
    Block, BlockContent, BlockProperties, ComponentActorOpen, CustomField, OpenComponent,
    OpenSubobject, Reference, compose_component_actor_open,
};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::packet::{BitWriter, Payload};
use icarus_bevy::experimental::rep_layout::{
    MovementLevel, RepCommand, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
    pawn_rep_layout, read_rep_properties,
};
use icarus_bevy::experimental::rep_rpc::read_field_header;
use icarus_bevy::experimental::session_open::{
    ActorKey, ConnectionExports, NetGuidCache, PathSegment,
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

fn layout() -> Vec<icarus_bevy::experimental::rep_layout::RepCommand> {
    pawn_rep_layout(
        MovementLevel::Whole,
        MovementLevel::Whole,
        RotationLevel::Bytes,
    )
}

fn bits(count: usize) -> Payload {
    let mut writer = BitWriter::new();
    for index in 0..count {
        writer.write_bit(index % 3 == 0);
    }
    Payload::from_writer(writer)
}

fn component(name: &str, checksum: u32, block: Option<BlockContent>) -> OpenComponent {
    OpenComponent {
        name: name.to_owned(),
        checksum,
        block: block.map(Into::into),
    }
}

const PAWN: ActorKey = ActorKey::Session(5);

/// An authored pawn in the shape of F-797 (not release data): `APawn`
/// handles 13 `Owner`, 15 `Instigator`, 17 `PlayerState`, 18 `Controller`.
fn open() -> ComponentActorOpen {
    ComponentActorOpen {
        key: PAWN,
        archetype: vec![
            PathSegment::new("Default__TestPawn_C", 0x1234, false),
            PathSegment::new("/Game/Test/TestPawn", 0, false),
        ],
        level: vec![
            PathSegment::new("PersistentLevel", 0, true),
            PathSegment::new("TestMap", 0, true),
            PathSegment::new("/Game/Maps/TestMap", 0, true),
        ],
        location: Some([100.0, -20.0, 5.0]),
        layout: layout(),
        references: BTreeMap::from([
            (13, Reference::Guid(900)),
            (15, Reference::Itself),
            (17, Reference::Component(2)),
            (18, Reference::Actor(ActorKey::World(77))),
        ]),
        values: BTreeMap::from([(5, RepValue::Byte(2)), (14, RepValue::Byte(3))]),
        components: vec![
            component(
                "Stats",
                11,
                Some(BlockContent {
                    has_rep_layout: false,
                    payload: bits(40),
                }),
            ),
            component("Silent", 12, None),
            component("State", 13, Some(BlockContent::create_only())),
        ],
        subobjects: vec![
            OpenSubobject {
                class: vec![
                    PathSegment::new("Bag", 21, false),
                    PathSegment::new("/Script/Test", 0, false),
                ],
                block: BlockContent {
                    has_rep_layout: true,
                    payload: bits(9),
                }
                .into(),
            },
            OpenSubobject {
                class: vec![
                    PathSegment::new("Bag", 21, false),
                    PathSegment::new("/Script/Test", 0, false),
                ],
                block: BlockContent {
                    has_rep_layout: true,
                    payload: bits(3),
                }
                .into(),
            },
        ],
        engine_net_version: 17,
    }
}

#[test]
fn guids_and_exports_follow_serialization_order() {
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let (composed, guids) =
        compose_component_actor_open(&mut cache, &mut exports, &open(), 3, ByteOrder::Little)
            .unwrap();
    // Actor 2; the referenced component (State) at handle 17 takes 4, the
    // referenced world actor 6, then Stats at its block 8; Silent has no
    // block and no reference, so it is never serialized; subobjects 10, 12.
    assert_eq!(guids.actor, 2);
    assert_eq!(guids.components, [Some(8), None, Some(4)]);
    assert_eq!(guids.subobjects, [10, 12]);
    assert_eq!(cache.actor_guid(ActorKey::World(77)), 6);
    // Exports: archetype, level, State, Stats, then the subobject class once.
    let mut map = PackageMapState::new();
    let decoded = map
        .consume_export_prefix(&mut composed.exports.unwrap().reader(), &codec())
        .unwrap();
    let top: Vec<u32> = decoded.iter().map(|reference| reference.guid).collect();
    assert_eq!(top.len(), 5);
    assert_eq!(&top[2..4], [4, 8]);
    assert_eq!(
        map.resolve_full_path(top[4]).as_deref(),
        Some("/Script/Test.Bag")
    );
    let body = read_actor_bunch_body(&mut composed.bunch.payload.reader(), true, &codec()).unwrap();
    assert!(body.spawn.unwrap().dynamic.unwrap().location.is_some());
    let rep_codec = RepLayoutCodec {
        actor: codec(),
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let stream =
        read_rep_properties(&mut body.blocks[0].payload.reader(), &layout(), &rep_codec).unwrap();
    let objects: Vec<(u32, u32)> = stream
        .entries
        .iter()
        .filter_map(|entry| match &entry.value {
            RepValue::Object(reference) => Some((entry.handle, reference.guid)),
            _ => None,
        })
        .collect();
    assert_eq!(objects, [(13, 900), (15, 2), (17, 4), (18, 6)]);
    // Blocks: actor, Stats, State (component order), then both subobjects
    // naming the class; contents unchanged.
    assert_eq!(body.blocks.len(), 5);
    let targets: Vec<(u32, bool)> = body.blocks[1..]
        .iter()
        .map(|block| match &block.target {
            BlockTarget::SubObject { object, class } => {
                (object.guid, matches!(class, SubObjectClass::Class(_)))
            }
            BlockTarget::Actor => panic!("a second actor block"),
        })
        .collect();
    assert_eq!(targets, [(8, false), (4, false), (10, true), (12, true)]);
    assert_eq!(body.blocks[1].payload, bits(40));
    assert!(!body.blocks[2].has_rep_layout && body.blocks[2].payload.bit_len() == 0);
    assert_eq!(body.blocks[4].payload, bits(3));
}

#[test]
fn a_second_connection_gets_the_same_objects_and_its_own_exports() {
    let mut cache = NetGuidCache::new();
    let mut first = ConnectionExports::new();
    let (one, one_guids) =
        compose_component_actor_open(&mut cache, &mut first, &open(), 3, ByteOrder::Little)
            .unwrap();
    let mut second = ConnectionExports::new();
    let (two, two_guids) =
        compose_component_actor_open(&mut cache, &mut second, &open(), 3, ByteOrder::Little)
            .unwrap();
    assert_eq!(one_guids, two_guids);
    assert_eq!(one.exports, two.exports);
    assert_eq!(one.bunch.payload, two.bunch.payload);
    // Acked on the first connection: no exports there any more.
    first.commit_bunch(1);
    first.received_ack(1);
    let (again, _) =
        compose_component_actor_open(&mut cache, &mut first, &open(), 3, ByteOrder::Little)
            .unwrap();
    assert!(again.exports.is_none());
    assert_eq!(again.bunch.payload, one.bunch.payload);
}

#[test]
fn invalid_references_and_duplicate_components_fail_before_any_assignment() {
    for broken in [
        {
            let mut open = open();
            open.references.insert(5, Reference::Itself);
            open
        },
        {
            let mut open = open();
            open.references.insert(16, Reference::Component(3));
            open
        },
        {
            let mut open = open();
            open.components[1].name = "Stats".to_owned();
            open
        },
    ] {
        let mut cache = NetGuidCache::new();
        let mut exports = ConnectionExports::new();
        assert!(
            compose_component_actor_open(&mut cache, &mut exports, &broken, 3, ByteOrder::Little)
                .is_err()
        );
        assert_eq!(cache.assign_dynamic(), 2, "nothing was assigned");
    }
}

fn rep_codec() -> RepLayoutCodec {
    RepLayoutCodec {
        actor: codec(),
        checksums: false,
        limits: RepLayoutLimits::default(),
    }
}

/// An authored composed block: a bool, an object reference and an int on a
/// three-command layout, then `fields` under a field bound of 6.
fn composed(references: BTreeMap<u32, Reference>, fields: Vec<CustomField>) -> Block {
    Block::Composed(BlockProperties {
        layout: vec![RepCommand::Bool, RepCommand::Object, RepCommand::Int],
        references,
        values: BTreeMap::from([(1, RepValue::Bool(true)), (3, RepValue::Int(-7))]),
        fields,
        field_bound: 6,
    })
}

#[test]
fn composed_blocks_resolve_their_references_where_they_are_serialized() {
    let mut open = open();
    // The first subobject references the unserialized `Silent` component and
    // ends with one field; the `State` component carries only a field.
    open.subobjects[0].block = composed(
        BTreeMap::from([(2, Reference::Component(1))]),
        vec![CustomField {
            index: 4,
            payload: bits(11),
        }],
    );
    open.components[2].block = Some(Block::Composed(BlockProperties {
        layout: vec![RepCommand::Bool],
        references: BTreeMap::new(),
        values: BTreeMap::new(),
        fields: vec![CustomField {
            index: 5,
            payload: bits(2),
        }],
        field_bound: 6,
    }));
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let (composed, guids) =
        compose_component_actor_open(&mut cache, &mut exports, &open, 3, ByteOrder::Little)
            .unwrap();
    // Silent is serialized at the subobject's reference, after the
    // subobject's class and GUID (10), so it takes 12 and the second
    // subobject 14.
    assert_eq!(guids.components, [Some(8), Some(12), Some(4)]);
    assert_eq!(guids.subobjects, [10, 14]);
    let mut map = PackageMapState::new();
    let decoded = map
        .consume_export_prefix(&mut composed.exports.unwrap().reader(), &codec())
        .unwrap();
    let top: Vec<u32> = decoded.iter().map(|reference| reference.guid).collect();
    assert_eq!(top.len(), 6);
    assert_eq!(map.resolve_path(top[5]), Some("Silent"));
    let body = read_actor_bunch_body(&mut composed.bunch.payload.reader(), true, &codec()).unwrap();
    // State: no properties, so no rep layout, and its one field.
    let state = &body.blocks[2];
    assert!(!state.has_rep_layout);
    let mut reader = state.payload.reader();
    let field = read_field_header(&mut reader, 6, 64).unwrap();
    assert!(reader.at_end());
    assert_eq!((field.index, field.payload), (5, bits(2)));
    // The subobject: its properties with the reference resolved, then the
    // field.
    let bag = &body.blocks[3];
    assert!(bag.has_rep_layout);
    let mut reader = bag.payload.reader();
    let stream = read_rep_properties(
        &mut reader,
        &[RepCommand::Bool, RepCommand::Object, RepCommand::Int],
        &rep_codec(),
    )
    .unwrap();
    let values: Vec<(u32, RepValue)> = stream
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect();
    assert_eq!(values[0], (1, RepValue::Bool(true)));
    assert!(matches!(&values[1], (2, RepValue::Object(object)) if object.guid == 12));
    assert_eq!(values[2], (3, RepValue::Int(-7)));
    let field = read_field_header(&mut reader, 6, 64).unwrap();
    assert!(reader.at_end());
    assert_eq!((field.index, field.payload), (4, bits(11)));
}

#[test]
fn invalid_block_contents_fail_before_any_assignment() {
    for block in [
        composed(BTreeMap::from([(2, Reference::Component(9))]), Vec::new()),
        composed(BTreeMap::from([(1, Reference::Itself)]), Vec::new()),
    ] {
        let mut open = open();
        open.subobjects[1].block = block;
        let mut cache = NetGuidCache::new();
        let mut exports = ConnectionExports::new();
        assert!(
            compose_component_actor_open(&mut cache, &mut exports, &open, 3, ByteOrder::Little)
                .is_err()
        );
        assert_eq!(cache.assign_dynamic(), 2, "nothing was assigned");
    }
    // A field index at its bound fails at encoding.
    let mut open = open();
    open.subobjects[1].block = composed(
        BTreeMap::new(),
        vec![CustomField {
            index: 6,
            payload: bits(1),
        }],
    );
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    assert!(
        compose_component_actor_open(&mut cache, &mut exports, &open, 3, ByteOrder::Little)
            .is_err()
    );
}
