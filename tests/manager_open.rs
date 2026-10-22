//! World manager opens (F-859) from the built-in release plans, pinned to
//! the bit lengths and SHA-256 digests of the opens M-707 captured (taken
//! from the capture, not from the composer), composed in the capture's GUID
//! state.
use icarus_bevy::experimental::{
    archive::ByteOrder,
    manager_open::{ManagerSlot, compose_manager_open},
    packet::Payload,
    release::ReleaseProfile,
    session_open::{ActorKey, ConnectionExports, NetGuidCache, PathSegment},
};
use sha2::{Digest, Sha256};

/// One captured manager open and the GUIDs assigned before it.
struct Captured {
    slot: ManagerSlot,
    channel: u32,
    actor: u32,
    archetype: u32,
    /// The archetype's outer package (native managers only).
    outer: Option<u32>,
    first_component: u32,
    exports_bits: usize,
    exports_sha256: &'static str,
    body_bits: usize,
    body_sha256: &'static str,
}

const CAPTURED: [Captured; 3] = [
    Captured {
        slot: ManagerSlot::Radiation,
        channel: 124,
        actor: 78,
        archetype: 817,
        outer: Some(23),
        first_component: 3078,
        exports_bits: 761,
        exports_sha256: "a736b2f1dfaa77e8961558cf72f05dc3e9db84aefaa0776969b500e1bb756277",
        body_bits: 204,
        body_sha256: "208ead61343d15e8d5b403f6330109de72af1267a999701768b238dcd0eef7dc",
    },
    Captured {
        slot: ManagerSlot::WorldTalent,
        channel: 125,
        actor: 76,
        archetype: 819,
        outer: Some(23),
        first_component: 3082,
        exports_bits: 777,
        exports_sha256: "f1010d7635f366293b1ca853859e6644130309033e9846253d48f59e0fa6e882",
        body_bits: 204,
        body_sha256: "9de9da14ab53bfe8100b8951565cb304c66e728deec2278d3a3f5f2e77f0c83f",
    },
    Captured {
        slot: ManagerSlot::Disaster,
        channel: 127,
        actor: 86,
        archetype: 823,
        outer: None,
        first_component: 3090,
        exports_bits: 1089,
        exports_sha256: "0025c91eae09f73d408464dda887d39c436c675e5d2d945265423744357de7bd",
        body_bits: 104,
        body_sha256: "3a8be5ffbbaffc0ae7cd03cbd32c937fd8b89731e74db1fb0d4b6c1a9e8e12ca",
    },
];

fn sha256(payload: &Payload) -> String {
    Sha256::digest(payload.bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Assign static GUIDs until the next one is `next`.
fn burn_static_to(cache: &mut NetGuidCache, next: u32) {
    for index in 0.. {
        let probe = cache.assign_static(&[PathSegment::new(&format!("burn{index}"), 0, false)]);
        if probe + 2 >= next {
            assert_eq!(probe + 2, next, "static GUID {next} unreachable");
            return;
        }
    }
}

#[test]
fn manager_opens_match_the_captured_bit_lengths_and_digests() {
    let plans = ReleaseProfile::release_3_0_31()
        .manager_open_plans("/Game/Maps/Terrain_016_OLY/Terrain_016")
        .unwrap();
    for captured in &CAPTURED {
        let slot = captured.slot;
        let plan = &plans
            .iter()
            .find(|(candidate, _)| *candidate == slot)
            .unwrap()
            .1;
        // The level and the native archetype's outer package were exported
        // and acked; the actor's GUID was assigned at the GameState.
        let mut cache = NetGuidCache::new();
        let mut exports = ConnectionExports::new();
        cache.export_static(&mut exports, &plan.level).unwrap();
        if let Some(outer) = captured.outer {
            burn_static_to(&mut cache, outer);
            let held = cache
                .export_static(&mut exports, &plan.archetype[1..])
                .unwrap();
            assert_eq!(held.guid, outer);
        }
        burn_static_to(&mut cache, captured.archetype);
        while cache.assign_dynamic() + 2 < captured.actor {}
        let key = ActorKey::World(1);
        assert_eq!(cache.actor_guid(key), captured.actor);
        while cache.assign_dynamic() + 2 < captured.first_component {}
        exports.commit_bunch(1);
        exports.received_ack(1);
        let open = compose_manager_open(
            &mut cache,
            &mut exports,
            plan,
            key,
            captured.channel,
            ByteOrder::Little,
        )
        .unwrap();
        let open_exports = open.exports.unwrap();
        assert_eq!(open_exports.bit_len(), captured.exports_bits, "{slot:?}");
        assert_eq!(sha256(&open_exports), captured.exports_sha256, "{slot:?}");
        assert_eq!(open.bunch.payload.bit_len(), captured.body_bits, "{slot:?}");
        assert_eq!(
            sha256(&open.bunch.payload),
            captured.body_sha256,
            "{slot:?}"
        );
    }
}

/// Saved quest state (`C018`) composes into the QuestManager open (F-861):
/// the faction mission's row, nondefault difficulty and flags, and the
/// initial quest and keyed persistent actors by their Icarus actor GUIDs'
/// keys; defaults and links without a valid GUID are left out.
#[test]
fn saved_quest_state_composes_into_the_quest_manager_open() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        control_messages::ArchiveText,
        manager_open::quest_handle,
        prospect_info::{RelevantQuestActor, SavedQuestManager},
        rep_layout::{
            MovementLevel, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
            quest_manager_rep_layout, read_rep_properties,
        },
    };
    use std::collections::BTreeMap;
    let units = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    let profile = ReleaseProfile::release_3_0_32();
    let saved = SavedQuestManager {
        faction_mission: units("Mission_C"),
        initial_quest: Some(9_001),
        mission_complete: false,
        run_quests: true,
        dynamic_quest_difficulty: 2,
        dynamic_quest_delay: 0.0,
        persistent_actors: vec![
            RelevantQuestActor {
                key: units("LAB_BASE"),
                icarus_uid: 501,
            },
            RelevantQuestActor {
                key: units("LAB_GONE"),
                icarus_uid: 0,
            },
        ],
        persistent_characters: Vec::new(),
    };
    let plan = profile
        .manager_open_plan(ManagerSlot::Quest, "/Game/Maps/Test/Test")
        .unwrap()
        .unwrap()
        .with_quest_state(&saved, 3)
        .unwrap();
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let open = compose_manager_open(
        &mut cache,
        &mut exports,
        &plan,
        ManagerSlot::Quest.key(),
        9,
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
    assert_eq!(body.blocks.len(), 3);
    let values: BTreeMap<u32, RepValue> = read_rep_properties(
        &mut body.blocks[0].payload.reader(),
        &quest_manager_rep_layout(
            MovementLevel::Whole,
            MovementLevel::Whole,
            RotationLevel::Bytes,
            profile.tags.tag_encoding(),
        ),
        &RepLayoutCodec {
            actor: codec,
            checksums: false,
            limits: RepLayoutLimits::default(),
        },
    )
    .unwrap()
    .entries
    .into_iter()
    .map(|entry| (entry.handle, entry.value))
    .collect();
    let sent: Vec<u32> = values.keys().copied().collect();
    assert_eq!(sent, [5, 14, 16, 17, 19, 20, 21, 24, 25]);
    assert_eq!(values[&quest_handle::FACTION_MISSION], RepValue::Byte(3));
    assert_eq!(
        values[&quest_handle::DYNAMIC_QUEST_DIFFICULTY],
        RepValue::Enum(2)
    );
    assert_eq!(values[&quest_handle::RUN_QUESTS], RepValue::Bool(true));
    let RepValue::Object(initial) = &values[&quest_handle::INITIAL_QUEST] else {
        panic!("an initial quest reference");
    };
    assert_eq!(initial.guid, cache.actor_guid(ActorKey::Saved(9_001)));
    let RepValue::DynamicArray(actors) = &values[&quest_handle::RELEVANT_PERSISTENT_ACTORS] else {
        panic!("relevant persistent actors");
    };
    assert_eq!(actors.count, 1);
    assert_eq!(
        actors.stream.entries[0].value,
        RepValue::Str(ArchiveText::new(&units("LAB_BASE")).unwrap())
    );
    let RepValue::Object(base) = &actors.stream.entries[1].value else {
        panic!("a persistent actor reference");
    };
    assert_eq!(base.guid, cache.actor_guid(ActorKey::Saved(501)));
    assert_ne!(base.guid, initial.guid);
    // Saved links never share a GUID with a domain actor of the same id.
    assert_ne!(cache.actor_guid(ActorKey::World(501)), base.guid);
}

/// Saved container inventories compose into the InventoryContainerManager
/// open (F-866): `Inventories` lists the runtime subobjects' GUIDs in block
/// order, and each block carries its info row and its slots, an item's row,
/// dynamic data and named stats included.
#[test]
fn container_inventories_compose_as_listed_subobjects() {
    use icarus_bevy::experimental::{
        actor_channel::{
            ActorChannelCodec, ActorChannelLimits, BlockTarget, read_actor_bunch_body,
        },
        control_messages::ArchiveText,
        manager_open::CONTAINER_INVENTORIES_HANDLE,
        pawn_open::{FastArrayContent, InventorySlot, PawnInventory, SlotItem},
        rep_layout::{NetworkName, RepLayoutCodec, RepLayoutLimits, RepValue, read_rep_properties},
        rep_rpc::{
            FAST_ARRAY_COUNT_CAP, FastArrayItem, FastArrayItemLayout, read_custom_delta_header,
            read_fast_array_delta, read_field_header,
        },
    };
    let units = |text: &str| text.encode_utf16().collect::<Vec<u16>>();
    let profile = ReleaseProfile::release_3_0_32();
    let item = SlotItem {
        row: 42,
        dynamic: vec![(7, 15)],
        stats: vec![(ArchiveText::new(&units("Potency")).unwrap(), 3)],
    };
    let inventories = vec![
        PawnInventory {
            info_row: 9,
            slots: FastArrayContent {
                replication_key: 3,
                items: vec![InventorySlot {
                    item: Some(item),
                    ..InventorySlot::empty(5, 0)
                }],
            },
        },
        PawnInventory {
            info_row: 10,
            slots: FastArrayContent {
                replication_key: 2,
                items: vec![InventorySlot::empty(6, 0)],
            },
        },
    ];
    let plan = profile
        .manager_open_plan(ManagerSlot::InventoryContainer, "/Game/Maps/Test/Test")
        .unwrap()
        .unwrap()
        .with_container_inventories(inventories)
        .unwrap();
    let key = ManagerSlot::InventoryContainer.key();
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let open =
        compose_manager_open(&mut cache, &mut exports, &plan, key, 10, ByteOrder::Little).unwrap();
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
    let body = read_actor_bunch_body(&mut open.bunch.payload.reader(), true, &codec).unwrap();
    assert_eq!(body.blocks.len(), 3);
    let actor = read_rep_properties(
        &mut body.blocks[0].payload.reader(),
        &plan.layout,
        &rep_codec,
    )
    .unwrap();
    let Some(RepValue::DynamicArray(listed)) = actor
        .entries
        .iter()
        .find(|entry| entry.handle == CONTAINER_INVENTORIES_HANDLE)
        .map(|entry| entry.value.clone())
    else {
        panic!("an Inventories array");
    };
    let listed: Vec<u32> = listed
        .stream
        .entries
        .iter()
        .map(|entry| match &entry.value {
            RepValue::Object(reference) => reference.guid,
            other => panic!("not an object: {other:?}"),
        })
        .collect();
    let blocks = &plan.inventories.as_ref().unwrap().blocks;
    for (index, block) in body.blocks[1..].iter().enumerate() {
        let BlockTarget::SubObject { object, .. } = &block.target else {
            panic!("a subobject block");
        };
        assert_eq!(object.guid, listed[index]);
        assert_eq!(object.guid, cache.subobject_guid(key, index as u32));
        let mut reader = block.payload.reader();
        let properties =
            read_rep_properties(&mut reader, &blocks.inventory_layout, &rep_codec).unwrap();
        assert!(
            properties
                .entries
                .iter()
                .any(|entry| entry.handle == 7 && entry.value == RepValue::Byte(9 + index as u8))
        );
        let field = read_field_header(
            &mut reader,
            blocks.inventory_bound,
            rep_codec.limits.max_opaque_bits,
        )
        .unwrap();
        let mut field = field.payload.reader();
        read_custom_delta_header(&mut field, profile.engine_network_version, 1).unwrap();
        let delta = read_fast_array_delta(
            &mut field,
            &FastArrayItemLayout::Delta(blocks.slot_layout.clone()),
            &rep_codec,
            FAST_ARRAY_COUNT_CAP,
        )
        .unwrap();
        assert_eq!(delta.array_replication_key, 3 - index as i32);
        let FastArrayItem::Delta(slot) = &delta.changed[0].1 else {
            panic!("a delta item");
        };
        let value = |handle: u32| {
            slot.entries
                .iter()
                .find(|entry| entry.handle == handle)
                .map(|entry| entry.value.clone())
        };
        if index == 0 {
            assert_eq!(value(2), Some(RepValue::UInt16(42)));
            assert_eq!(value(16), Some(RepValue::UInt16(42)));
            assert_eq!(value(10), Some(RepValue::Bool(true)));
            let Some(RepValue::DynamicArray(dynamic)) = value(3) else {
                panic!("dynamic data");
            };
            assert_eq!(dynamic.stream.entries[0].value, RepValue::Enum(7));
            assert_eq!(dynamic.stream.entries[1].value, RepValue::Int(15));
            let Some(RepValue::DynamicArray(stats)) = value(7) else {
                panic!("stats");
            };
            assert_eq!(
                stats.stream.entries[0].value,
                RepValue::NetworkName(NetworkName::String {
                    text: ArchiveText::new(&units("Potency")).unwrap(),
                    number: 0,
                })
            );
        } else {
            assert_eq!(value(2), Some(RepValue::UInt16(u16::MAX)));
            assert_eq!(value(3), None);
            assert_eq!(value(10), Some(RepValue::Bool(false)));
        }
    }
}

/// A saved FLOD tile count composes into the FLOD manager open (F-914):
/// its tile array references that many tile actors, keyed by position, at
/// the captured spawn location; a negative count fails.
#[test]
fn flod_tiles_compose_as_keyed_tile_references() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        manager_open::FLOD_TILES_HANDLE,
        rep_layout::{RepLayoutCodec, RepLayoutLimits, RepValue, read_rep_properties},
    };
    let profile = ReleaseProfile::release_3_0_32();
    let base = profile
        .manager_open_plan(ManagerSlot::Flod, "/Game/Maps/Test/Test")
        .unwrap()
        .unwrap();
    assert_eq!(base.location, Some([0.0, 0.0, -100_000.0]));
    for bad in [-1, i32::from(u16::MAX) + 1, i32::MAX] {
        assert!(base.clone().with_flod_tiles(bad).is_err(), "{bad}");
    }
    assert!(
        base.clone()
            .with_flod_tiles(0)
            .unwrap()
            .actor_arrays
            .is_empty()
    );
    let plan = base.with_flod_tiles(3).unwrap();
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let key = ManagerSlot::Flod.key();
    let open =
        compose_manager_open(&mut cache, &mut exports, &plan, key, 6, ByteOrder::Little).unwrap();
    let codec = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let body = read_actor_bunch_body(&mut open.bunch.payload.reader(), true, &codec).unwrap();
    assert_eq!(body.blocks.len(), 1);
    assert!(body.spawn.unwrap().dynamic.unwrap().location.is_some());
    let values = read_rep_properties(
        &mut body.blocks[0].payload.reader(),
        &plan.layout,
        &RepLayoutCodec {
            actor: codec,
            checksums: false,
            limits: RepLayoutLimits::default(),
        },
    )
    .unwrap();
    let Some(RepValue::DynamicArray(tiles)) = values
        .entries
        .into_iter()
        .find_map(|entry| (entry.handle == FLOD_TILES_HANDLE).then_some(entry.value))
    else {
        panic!("a tile array");
    };
    let guids: Vec<u32> = tiles
        .stream
        .entries
        .iter()
        .map(|entry| match &entry.value {
            RepValue::Object(reference) => reference.guid,
            other => panic!("not an object: {other:?}"),
        })
        .collect();
    let expected: Vec<u32> = (0..3)
        .map(|index| cache.actor_guid(ActorKey::FlodTile(index)))
        .collect();
    assert_eq!(guids, expected);
}
