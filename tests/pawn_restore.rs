use std::collections::BTreeMap;

use icarus_bevy::experimental::actor_channel::{
    ActorChannelCodec, ActorChannelLimits, BlockTarget, CompressedRotator, SubObjectClass,
    read_actor_bunch_body,
};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::pawn_open::{
    CharacterVitals, PawnContent, PawnOpenPlan, PawnOpenState, character_state_handle as h,
    compose_pawn_open, handle,
};
use icarus_bevy::experimental::pawn_restore::{
    CurrentPawnState, PawnRestoreGap, RestoredPawn, RestoredStomachContent,
    STOMACH_CONTENTS_HANDLE, clamp_health, clamp_level, compose_restored_pawn_update,
    restore_saved_pawn,
};
use icarus_bevy::experimental::recorders::{SavedPlayerState, SavedStomachContent};
use icarus_bevy::experimental::release::ReleaseProfile;
use icarus_bevy::experimental::rep_layout::{
    PropertyStream, RepArray, RepCommand, RepLayoutCodec, RepLayoutLimits, RepValue,
    player_character_state_rep_layout, read_rep_properties, stomach_component_rep_layout,
};
use icarus_bevy::experimental::session_open::{
    ActorKey, ComponentBlock, ComponentExport, ConnectionExports, NetGuidCache,
};

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Authored item-static rows (not release data).
const ROWS: [&str; 4] = ["Meat_Cooked", "Armour_Head_A", "Armour_Chest_B", "Berry"];

fn row_of(name: &[u16]) -> anyhow::Result<Option<usize>> {
    Ok(ROWS.iter().position(|row| units(row) == name))
}

/// A living, fully recorded character.
fn saved() -> SavedPlayerState {
    SavedPlayerState {
        location: Some([100.0, -200.0, 300.0]),
        rotation: Some([0.0, 90.0, 0.0]),
        is_alive: Some(true),
        health: Some(60),
        food_level: Some(50),
        water_level: Some(40),
        oxygen_level: Some(30),
        radiation_level: Some(20),
        stomach_contents: Vec::new(),
        respawn_count: Some(2),
        has_granted_loadout: Some(true),
        cosmetic_armour_overrides: Vec::new(),
    }
}

/// Authored current values with every bound the setters read.
fn current_state() -> BTreeMap<u32, RepValue> {
    BTreeMap::from([
        (h::HEALTH, RepValue::Int(100)),
        (h::MAX_HEALTH, RepValue::Int(100)),
        (h::OXYGEN, RepValue::Int(80)),
        (h::MIN_OXYGEN, RepValue::Int(0)),
        (h::MAX_OXYGEN, RepValue::Int(80)),
        (h::WATER, RepValue::Int(90)),
        (h::MIN_WATER, RepValue::Int(10)),
        (h::MAX_WATER, RepValue::Int(90)),
        (h::FOOD, RepValue::Int(70)),
        (h::MIN_FOOD, RepValue::Int(0)),
        (h::MAX_FOOD, RepValue::Int(70)),
        (h::RADIATION, RepValue::Int(0)),
        (h::MIN_RADIATION, RepValue::Int(0)),
        (h::MAX_RADIATION, RepValue::Int(100)),
    ])
}

fn restore(saved: &SavedPlayerState, state: &BTreeMap<u32, RepValue>) -> RestoredPawn {
    restore_with_stomach(saved, state, 0)
}

fn restore_with_stomach(
    saved: &SavedPlayerState,
    state: &BTreeMap<u32, RepValue>,
    stomach_contents: usize,
) -> RestoredPawn {
    let current = CurrentPawnState {
        character_state: state,
        stomach_contents,
    };
    restore_saved_pawn(saved, &current, row_of)
        .unwrap()
        .unwrap()
}

fn row_array(rows: &[u16]) -> RepValue {
    RepValue::DynamicArray(RepArray {
        count: u16::try_from(rows.len()).unwrap(),
        stream: PropertyStream {
            entries: rows
                .iter()
                .zip(1..)
                .map(
                    |(row, element)| icarus_bevy::experimental::rep_layout::RepEntry {
                        handle: element,
                        checksum: None,
                        value: RepValue::UInt16(*row),
                    },
                )
                .collect(),
            terminator_checksum: None,
        },
    })
}

#[test]
fn setters_clamp_at_both_ends() {
    assert_eq!(clamp_level(-5, 0, 80), 0);
    assert_eq!(clamp_level(0, 0, 80), 0);
    assert_eq!(clamp_level(79, 0, 80), 79);
    assert_eq!(clamp_level(80, 0, 80), 80);
    assert_eq!(clamp_level(500, 0, 80), 80);
    assert_eq!(clamp_level(5, 10, 90), 10);
    // Bounds out of order: below the minimum gives it, otherwise the maximum.
    assert_eq!(clamp_level(5, 10, 0), 10);
    assert_eq!(clamp_level(15, 10, 0), 0);
    assert_eq!(clamp_health(-1, 100), 0);
    assert_eq!(clamp_health(0, 100), 0);
    assert_eq!(clamp_health(99, 100), 99);
    assert_eq!(clamp_health(100, 100), 100);
    assert_eq!(clamp_health(250, 100), 100);
}

#[test]
fn a_living_record_restores_pawn_values_and_clamped_levels() {
    let state = current_state();
    let mut record = saved();
    record.health = Some(250);
    record.oxygen_level = Some(500);
    record.water_level = Some(-7);
    record.food_level = Some(70);
    record.radiation_level = Some(20);
    let restored = restore(&record, &state);
    assert_eq!(restored.location, [100.0, -200.0, 300.0]);
    assert_eq!(
        restored.pawn,
        BTreeMap::from([
            (
                handle::REPLICATED_CONTROL_ROTATION,
                RepValue::Rotator(CompressedRotator::from_degrees(0.0, 90.0, 0.0).unwrap()),
            ),
            (handle::RESPAWN_COUNT, RepValue::Int(2)),
            (handle::HAS_GRANTED_LOADOUT, RepValue::Bool(true)),
        ])
    );
    // Health clamps to MaxHealth (unchanged, not sent) and the alive state
    // the pawn never sent goes out; oxygen clamps to its maximum (unchanged),
    // water to its minimum, food stays at its maximum (unchanged).
    assert_eq!(
        restored.character_state,
        BTreeMap::from([
            (h::CURRENT_ALIVE_STATE, RepValue::Enum(0)),
            (h::WATER, RepValue::Int(10)),
            (h::RADIATION, RepValue::Int(20)),
        ])
    );
    assert_eq!(restored.stomach, None);
    let debug = format!("{restored:?}");
    assert!(
        !debug.contains("100.0") && !debug.contains("Int"),
        "{debug}"
    );
}

#[test]
fn unrecorded_values_are_skipped() {
    let state = current_state();
    let record = SavedPlayerState {
        health: Some(-1),
        food_level: Some(-1),
        water_level: Some(-1),
        oxygen_level: Some(-1),
        radiation_level: Some(-1),
        ..saved()
    };
    let restored = restore(&record, &state);
    assert!(restored.character_state.is_empty());
    // Unrecorded levels need no bounds.
    let restored = restore(&record, &BTreeMap::new());
    assert!(restored.character_state.is_empty());
    assert_eq!(restored.pawn.len(), 3);
}

#[test]
fn health_below_one_gives_the_dead_alive_state() {
    let state = current_state();
    for (saved_health, health) in [(0, 0), (-20, 0)] {
        let record = SavedPlayerState {
            health: Some(saved_health),
            ..saved()
        };
        let restored = restore(&record, &state);
        assert_eq!(restored.character_state[&h::HEALTH], RepValue::Int(health));
        assert_eq!(
            restored.character_state[&h::CURRENT_ALIVE_STATE],
            RepValue::Enum(1)
        );
    }
    let record = SavedPlayerState {
        health: Some(1),
        ..saved()
    };
    let restored = restore(&record, &state);
    assert_eq!(restored.character_state[&h::HEALTH], RepValue::Int(1));
    assert_eq!(
        restored.character_state[&h::CURRENT_ALIVE_STATE],
        RepValue::Enum(0)
    );
    // A current alive state equal to the restored one is not sent.
    let mut alive = state.clone();
    alive.insert(h::CURRENT_ALIVE_STATE, RepValue::Enum(0));
    assert!(
        !restore(&record, &alive)
            .character_state
            .contains_key(&h::CURRENT_ALIVE_STATE)
    );
}

#[test]
fn dead_records_are_unsupported() {
    let state = current_state();
    let current = CurrentPawnState {
        character_state: &state,
        stomach_contents: 0,
    };
    let gap = |record: SavedPlayerState| {
        restore_saved_pawn(&record, &current, row_of)
            .unwrap()
            .unwrap_err()
    };
    assert_eq!(
        gap(SavedPlayerState {
            is_alive: Some(false),
            ..saved()
        }),
        PawnRestoreGap::DeadOnSave
    );
    // A dead record needs no health or levels to be reported.
    assert_eq!(
        gap(SavedPlayerState {
            is_alive: Some(false),
            health: None,
            food_level: None,
            ..saved()
        }),
        PawnRestoreGap::DeadOnSave
    );
}

#[test]
fn omitted_fields_restore_as_the_recorders_constructed_defaults() {
    // F-963: alive, RespawnCount 0, HasGrantedLoadout false, Health and
    // levels -1 (left untouched).
    let state = current_state();
    let current = CurrentPawnState {
        character_state: &state,
        stomach_contents: 0,
    };
    let restored = restore_saved_pawn(&SavedPlayerState::default(), &current, row_of)
        .unwrap()
        .unwrap();
    assert!(restored.character_state.is_empty());
    assert_eq!(
        restored.pawn.get(&handle::RESPAWN_COUNT),
        Some(&RepValue::Int(0))
    );
    assert_eq!(
        restored.pawn.get(&handle::HAS_GRANTED_LOADOUT),
        Some(&RepValue::Bool(false))
    );
}

#[test]
fn level_bounds_default_to_the_constructed_state() {
    let mut state = current_state();
    state.insert(h::MAX_HEALTH, RepValue::Byte(1));
    let current = CurrentPawnState {
        character_state: &state,
        stomach_contents: 0,
    };
    assert!(restore_saved_pawn(&saved(), &current, row_of).is_err());
    // A fresh pawn's vitals carry no minima or radiation bounds: the
    // constructed defaults apply (F-963: minima 0, MaxRadiation 1,000).
    let fresh = CharacterVitals {
        health: 100,
        max_health: 100,
        stamina: 100,
        max_stamina: 100,
        oxygen: 80,
        max_oxygen: 80,
        water: 90,
        max_water: 90,
        food: 70,
        max_food: 70,
        biome_row: 0,
        growth_row: 0,
    }
    .values();
    let current = CurrentPawnState {
        character_state: &fresh,
        stomach_contents: 0,
    };
    let record = SavedPlayerState {
        oxygen_level: Some(-5),
        food_level: Some(20),
        water_level: Some(-1),
        radiation_level: Some(1_500),
        ..saved()
    };
    let restored = restore_saved_pawn(&record, &current, row_of)
        .unwrap()
        .unwrap();
    assert_eq!(
        restored.character_state.get(&h::OXYGEN),
        Some(&RepValue::Int(0))
    );
    assert_eq!(
        restored.character_state.get(&h::FOOD),
        Some(&RepValue::Int(20))
    );
    assert!(!restored.character_state.contains_key(&h::WATER));
    assert_eq!(
        restored.character_state.get(&h::RADIATION),
        Some(&RepValue::Int(1_000))
    );
}

#[test]
fn cosmetic_overrides_keep_valid_unique_rows() {
    let state = current_state();
    let record = SavedPlayerState {
        cosmetic_armour_overrides: [
            "Armour_Chest_B",
            "Missing",
            "Armour_Head_A",
            "Armour_Chest_B",
        ]
        .map(units)
        .to_vec(),
        ..saved()
    };
    let restored = restore(&record, &state);
    assert_eq!(
        restored.pawn[&handle::COSMETIC_ARMOUR_OVERRIDES],
        row_array(&[2, 1])
    );
    // No valid row leaves the pawn's list alone.
    let record = SavedPlayerState {
        cosmetic_armour_overrides: vec![units("Missing")],
        ..saved()
    };
    let restored = restore(&record, &state);
    assert!(
        !restored
            .pawn
            .contains_key(&handle::COSMETIC_ARMOUR_OVERRIDES)
    );
}

#[test]
fn the_stomach_keeps_entries_with_known_food() {
    let state = current_state();
    let record = SavedPlayerState {
        stomach_contents: vec![
            SavedStomachContent {
                food_row_name: units("Berry"),
                modifier_name: units("Modifier_A"),
            },
            SavedStomachContent {
                food_row_name: units("Unknown_Food"),
                modifier_name: units("Modifier_B"),
            },
            SavedStomachContent {
                food_row_name: Vec::new(),
                modifier_name: units("Modifier_C"),
            },
        ],
        ..saved()
    };
    let restored = restore(&record, &state);
    assert_eq!(
        restored.stomach,
        Some(vec![RestoredStomachContent {
            food_row: 3,
            modifier_name: units("Modifier_A"),
        }])
    );
    assert!(!format!("{restored:?}").contains("Modifier"));
    // An unresolved stomach clears a full one and leaves an empty one.
    let record = SavedPlayerState {
        stomach_contents: vec![SavedStomachContent {
            food_row_name: units("Unknown_Food"),
            modifier_name: Vec::new(),
        }],
        ..saved()
    };
    assert_eq!(restore(&record, &state).stomach, None);
    assert_eq!(
        restore_with_stomach(&record, &state, 2).stomach,
        Some(Vec::new())
    );
}

fn plan() -> (ReleaseProfile, PawnOpenPlan, usize) {
    let profile = ReleaseProfile::release_3_0_32();
    let plan = profile
        .pawn_open_plan("/Game/Maps/Terrain_016_OLY/Terrain_016")
        .unwrap()
        .unwrap();
    let stomach = plan
        .components
        .iter()
        .position(|component| component.name == "Stomach")
        .unwrap();
    (profile, plan, stomach)
}

/// The pawn's open, composed and acked.
fn opened(plan: &PawnOpenPlan, key: ActorKey) -> (NetGuidCache, ConnectionExports) {
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let state = PawnOpenState {
        key,
        controller: 2,
        player_state: 4,
        location: [0.0, 0.0, 0.0],
        content: PawnContent {
            character_state: current_state(),
            ..PawnContent::default()
        },
    };
    compose_pawn_open(&mut cache, &mut exports, plan, &state, 3, ByteOrder::Little).unwrap();
    exports.commit_bunch(1);
    exports.received_ack(1);
    (cache, exports)
}

fn component_guid(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &PawnOpenPlan,
    key: ActorKey,
    index: usize,
) -> u32 {
    let actor = cache.actor_guid(key);
    let reference = cache
        .export_keyed_subobject(
            exports,
            key,
            actor,
            &ComponentExport {
                name: plan.components[index].name.clone(),
                checksum: plan.components[index].checksum,
                block: ComponentBlock::None,
            },
        )
        .unwrap();
    assert!(reference.export.is_none());
    reference.guid
}

#[test]
fn the_restored_update_decodes_through_the_readers() {
    let (profile, plan, stomach) = plan();
    let key = ActorKey::Session(7);
    let (mut cache, mut exports) = opened(&plan, key);
    let record = SavedPlayerState {
        health: Some(0),
        oxygen_level: Some(500),
        cosmetic_armour_overrides: vec![units("Armour_Head_A")],
        ..saved()
    };
    let restored = restore_with_stomach(&record, &current_state(), 1);
    assert_eq!(restored.stomach, Some(Vec::new()));
    let payload = compose_restored_pawn_update(
        &mut cache,
        &mut exports,
        &plan,
        key,
        stomach,
        &restored,
        ByteOrder::Little,
    )
    .unwrap();
    assert_eq!(exports.bunch_exports().count(), 0);

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
    let body = read_actor_bunch_body(&mut payload.payload.reader(), false, &codec).unwrap();
    assert!(body.spawn.is_none());
    assert_eq!(body.blocks.len(), 3);
    let decode = |index: usize, layout: &[RepCommand]| {
        let block = &body.blocks[index];
        assert!(block.has_rep_layout);
        let mut reader = block.payload.reader();
        let stream = read_rep_properties(&mut reader, layout, &rep_codec).unwrap();
        assert!(reader.at_end());
        stream
            .entries
            .into_iter()
            .map(|entry| (entry.handle, entry.value))
            .collect::<BTreeMap<_, _>>()
    };

    assert_eq!(body.blocks[0].target, BlockTarget::Actor);
    assert_eq!(
        decode(0, &plan.layout),
        BTreeMap::from([
            (
                handle::REPLICATED_CONTROL_ROTATION,
                RepValue::Rotator(CompressedRotator::from_degrees(0.0, 90.0, 0.0).unwrap()),
            ),
            (handle::RESPAWN_COUNT, RepValue::Int(2)),
            (handle::HAS_GRANTED_LOADOUT, RepValue::Bool(true)),
            (handle::COSMETIC_ARMOUR_OVERRIDES, row_array(&[1])),
        ])
    );

    let actor_state = component_guid(&mut cache, &mut exports, &plan, key, plan.actor_state);
    let stomach_guid = component_guid(&mut cache, &mut exports, &plan, key, stomach);
    let BlockTarget::SubObject { object, class } = &body.blocks[1].target else {
        panic!("ActorState block");
    };
    assert_eq!(
        (object.guid, class),
        (actor_state, &SubObjectClass::StablyNamed)
    );
    assert_eq!(
        decode(1, &player_character_state_rep_layout()),
        BTreeMap::from([
            (h::HEALTH, RepValue::Int(0)),
            (h::CURRENT_ALIVE_STATE, RepValue::Enum(1)),
            (h::WATER, RepValue::Int(40)),
            (h::FOOD, RepValue::Int(50)),
            (h::RADIATION, RepValue::Int(20)),
        ])
    );

    let BlockTarget::SubObject { object, class } = &body.blocks[2].target else {
        panic!("stomach block");
    };
    assert_eq!(
        (object.guid, class),
        (stomach_guid, &SubObjectClass::StablyNamed)
    );
    assert_eq!(
        decode(2, &stomach_component_rep_layout()),
        BTreeMap::from([(
            STOMACH_CONTENTS_HANDLE,
            RepValue::DynamicArray(RepArray {
                count: 0,
                stream: PropertyStream {
                    entries: Vec::new(),
                    terminator_checksum: None,
                },
            }),
        )])
    );
}

#[test]
fn the_update_needs_an_acked_open_and_no_stomach_entries() {
    let (_, plan, stomach) = plan();
    let key = ActorKey::Session(7);
    let mut restored = restore(&saved(), &current_state());
    assert!(!restored.character_state.is_empty());

    // The open went out but is not acked: its components are still exported.
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let state = PawnOpenState {
        key,
        controller: 2,
        player_state: 4,
        location: [0.0, 0.0, 0.0],
        content: PawnContent::default(),
    };
    compose_pawn_open(
        &mut cache,
        &mut exports,
        &plan,
        &state,
        3,
        ByteOrder::Little,
    )
    .unwrap();
    exports.commit_bunch(1);
    let error = compose_restored_pawn_update(
        &mut cache,
        &mut exports,
        &plan,
        key,
        stomach,
        &restored,
        ByteOrder::Little,
    )
    .unwrap_err();
    assert!(error.to_string().contains("not acked"), "{error}");
    assert_eq!(exports.bunch_exports().count(), 0);

    let (mut cache, mut exports) = opened(&plan, key);
    restored.stomach = Some(vec![RestoredStomachContent {
        food_row: 3,
        modifier_name: units("Modifier_A"),
    }]);
    let error = compose_restored_pawn_update(
        &mut cache,
        &mut exports,
        &plan,
        key,
        stomach,
        &restored,
        ByteOrder::Little,
    )
    .unwrap_err();
    assert!(
        error.to_string().contains("AddSaveDataToStomach"),
        "{error}"
    );
    restored.stomach = None;
    assert!(
        compose_restored_pawn_update(
            &mut cache,
            &mut exports,
            &plan,
            key,
            plan.components.len(),
            &restored,
            ByteOrder::Little,
        )
        .is_err()
    );
    // Only the actor block when the character state is unchanged.
    restored.character_state.clear();
    let payload = compose_restored_pawn_update(
        &mut cache,
        &mut exports,
        &plan,
        key,
        stomach,
        &restored,
        ByteOrder::Little,
    )
    .unwrap();
    let codec = ActorChannelCodec {
        engine_net_version: plan.engine_net_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let body = read_actor_bunch_body(&mut payload.payload.reader(), false, &codec).unwrap();
    assert_eq!(body.blocks.len(), 1);
}

#[test]
fn restored_inventories_send_their_slot_deltas_on_the_runtime_subobjects() {
    use icarus_bevy::experimental::{
        control_messages::ArchiveText,
        inventory_restore::{InventoryRestore, SlotChange},
        pawn_open::{FastArrayContent, InventorySlot, PawnInventory, SlotItem},
        rep_rpc::{
            FAST_ARRAY_COUNT_CAP, FastArrayItem, FastArrayItemLayout, read_custom_delta_header,
            read_fast_array_delta, read_field_header,
        },
    };
    let (profile, plan, stomach) = plan();
    let key = ActorKey::Session(7);
    let empty = |count: i32| PawnInventory {
        info_row: 0,
        current_weight: None,
        slots: FastArrayContent {
            replication_key: 2,
            items: (0..count)
                .map(|index| InventorySlot::empty(9, index))
                .collect(),
        },
    };
    // The open carries two inventories; the second is restored.
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let state = PawnOpenState {
        key,
        controller: 2,
        player_state: 4,
        location: [0.0, 0.0, 0.0],
        content: PawnContent {
            character_state: current_state(),
            inventories: vec![empty(1), empty(2)],
            ..PawnContent::default()
        },
    };
    compose_pawn_open(
        &mut cache,
        &mut exports,
        &plan,
        &state,
        3,
        ByteOrder::Little,
    )
    .unwrap();
    exports.commit_bunch(1);
    exports.received_ack(1);

    let item = SlotItem {
        dynamic: vec![(7, 5)],
        guid: ArchiveText::new(&units("GUID")).unwrap(),
        owner_lookup_id: 0,
        ..SlotItem::new(40).unwrap()
    };
    let mut restored_slots = empty(2).slots.items;
    restored_slots[1].item = Some(item.clone());
    let added = InventorySlot {
        item: Some(item),
        ..InventorySlot::empty(9, 2)
    };
    restored_slots.push(added.clone());
    let inventory = InventoryRestore {
        inventory: 1,
        opened: empty(2),
        restored: PawnInventory {
            slots: FastArrayContent {
                replication_key: 9,
                items: restored_slots.clone(),
            },
            ..empty(2)
        },
        base_key: 4,
        changes: vec![
            (2, SlotChange::Changed(restored_slots[1].clone())),
            (3, SlotChange::Added(added)),
        ],
        weight: Some(250),
    };
    let restored = RestoredPawn {
        inventories: vec![inventory],
        ..restore(&SavedPlayerState::default(), &current_state())
    };
    let payload = compose_restored_pawn_update(
        &mut cache,
        &mut exports,
        &plan,
        key,
        stomach,
        &restored,
        ByteOrder::Little,
    )
    .unwrap();
    assert_eq!(exports.bunch_exports().count(), 0);

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
    let body = read_actor_bunch_body(&mut payload.payload.reader(), false, &codec).unwrap();
    let block = body.blocks.last().unwrap();
    let BlockTarget::SubObject { object, class } = &block.target else {
        panic!("inventory block");
    };
    // The second runtime subobject, named by its acked class.
    assert_eq!(object.guid, cache.subobject_guid(key, 1));
    let SubObjectClass::Class(class) = class else {
        panic!("class-named inventory");
    };
    assert!(exports_acked(&exports, class.guid));
    // The changed weight goes as a property ahead of the slots.
    assert!(block.has_rep_layout);
    let mut reader = block.payload.reader();
    let properties =
        read_rep_properties(&mut reader, &plan.content.inventory_layout, &rep_codec).unwrap();
    assert_eq!(
        properties
            .entries
            .iter()
            .map(|entry| (entry.handle, entry.value.clone()))
            .collect::<Vec<_>>(),
        [(4, RepValue::Int(250))]
    );
    let field = read_field_header(&mut reader, plan.content.inventory_bound, 1 << 20).unwrap();
    assert!(reader.at_end());
    assert_eq!(field.index, 4);
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
    assert_eq!(
        (delta.array_replication_key, delta.base_replication_key),
        (9, 4)
    );
    let handles = |index: usize| match &delta.changed[index].1 {
        FastArrayItem::Delta(stream) => stream
            .entries
            .iter()
            .map(|entry| entry.handle)
            .collect::<Vec<_>>(),
        FastArrayItem::Whole(_) => panic!("delta items"),
    };
    // A changed slot sends what differs from its open; an added one all.
    assert_eq!(delta.changed[0].0, 2);
    assert_eq!(handles(0), [2, 3, 10, 11, 12, 16]);
    assert_eq!(delta.changed[1].0, 3);
    assert_eq!(handles(1), [2, 3, 10, 11, 12, 13, 14, 15, 16, 17, 18]);
}

fn exports_acked(exports: &ConnectionExports, guid: u32) -> bool {
    exports.is_acked(guid)
}

#[test]
fn restored_modifiers_are_class_named_subobjects_with_their_classes_exported() {
    use icarus_bevy::experimental::modifier_restore::{MODIFIER_CLASS_CHECKSUM, RestoredModifier};
    use icarus_bevy::experimental::rep_layout::modifier_state_component_rep_layout;
    use icarus_bevy::experimental::session_open::PathSegment;

    let (profile, plan, stomach) = plan();
    let key = ActorKey::Session(7);
    let (mut cache, mut exports) = opened(&plan, key);
    let modifier = |uid: i32, final_effectiveness: i32, remaining_tenths: u16| RestoredModifier {
        row: 40,
        uid,
        lifetime: 300.0,
        final_effectiveness,
        remaining_tenths,
        class: vec![
            PathSegment::new("ModifierStateComponent", MODIFIER_CLASS_CHECKSUM, false),
            PathSegment::new("/Script/Icarus", 0, false),
        ],
        subobject: u32::try_from(uid).unwrap() - 1,
    };
    let restored = RestoredPawn {
        modifiers: vec![modifier(1, -31, 2954), modifier(2, 0, 0)],
        ..restore(&SavedPlayerState::default(), &current_state())
    };
    let update = compose_restored_pawn_update(
        &mut cache,
        &mut exports,
        &plan,
        key,
        stomach,
        &restored,
        ByteOrder::Little,
    )
    .unwrap();
    // The class is new to the connection: exported once, ahead of the body.
    assert!(update.exports.is_some());

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
    let body = read_actor_bunch_body(&mut update.payload.reader(), false, &codec).unwrap();
    let blocks = &body.blocks[body.blocks.len() - 2..];
    let mut classes = Vec::new();
    for (index, block) in blocks.iter().enumerate() {
        let BlockTarget::SubObject { object, class } = &block.target else {
            panic!("modifier block");
        };
        assert_eq!(
            object.guid,
            cache.subobject_guid(key, u32::try_from(index).unwrap())
        );
        let SubObjectClass::Class(class) = class else {
            panic!("class-named modifier");
        };
        assert!(!exports_acked(&exports, class.guid));
        classes.push(class.guid);
        assert!(block.has_rep_layout);
        let mut reader = block.payload.reader();
        let stream = read_rep_properties(
            &mut reader,
            &modifier_state_component_rep_layout(),
            &rep_codec,
        )
        .unwrap();
        assert!(reader.at_end());
        let values = stream
            .entries
            .into_iter()
            .map(|entry| (entry.handle, entry.value))
            .collect::<BTreeMap<_, _>>();
        assert_eq!(values[&2], RepValue::Bool(true));
        assert_eq!(values[&3], RepValue::UInt16(40));
        let RepValue::Object(owner) = &values[&4] else {
            panic!("owner");
        };
        assert_eq!(owner.guid, cache.actor_guid(key));
        assert_eq!(values[&5], RepValue::Int(i32::try_from(index).unwrap() + 1));
        assert_eq!(values[&6], RepValue::Float(300.0_f32.to_bits()));
        // Zero effectiveness and remaining time stay at the defaults.
        if index == 0 {
            assert_eq!(values[&7], RepValue::Int(-31));
            assert_eq!(values[&8], RepValue::UInt16(2954));
        } else {
            assert!(!values.contains_key(&7) && !values.contains_key(&8));
        }
    }
    assert_eq!(classes[0], classes[1]);
}

#[test]
fn restored_stats_send_their_delta_on_the_stat_container_first() {
    use icarus_bevy::experimental::{
        rep_layout::StatPairRepState,
        rep_rpc::{
            FAST_ARRAY_COUNT_CAP, FastArrayItem, FastArrayItemLayout, read_custom_delta_header,
            read_fast_array_delta, read_field_header,
        },
        stat_restore::StatDelta,
    };
    let (profile, plan, stomach) = plan();
    let key = ActorKey::Session(7);
    let (mut cache, mut exports) = opened(&plan, key);
    let pair = |stat, value| StatPairRepState { stat, value };
    let restored = RestoredPawn {
        stats: Some(StatDelta {
            base_key: 58,
            array_key: 60,
            changed: vec![(1, pair(0, 614)), (59, pair(46, 53))],
        }),
        ..restore(&saved(), &current_state())
    };
    let update = compose_restored_pawn_update(
        &mut cache,
        &mut exports,
        &plan,
        key,
        stomach,
        &restored,
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
    let body = read_actor_bunch_body(&mut update.payload.reader(), false, &codec).unwrap();
    // The pawn's own block, then the stat container's, then ActorState's.
    assert!(matches!(body.blocks[0].target, BlockTarget::Actor));
    let stat_guid = component_guid(&mut cache, &mut exports, &plan, key, plan.stat_container);
    let block = &body.blocks[1];
    let BlockTarget::SubObject { object, class } = &block.target else {
        panic!("stat container block");
    };
    assert_eq!(object.guid, stat_guid);
    assert!(matches!(class, SubObjectClass::StablyNamed));
    assert!(!block.has_rep_layout);
    let mut reader = block.payload.reader();
    let field = read_field_header(&mut reader, plan.content.stat_container_bound, 1 << 20).unwrap();
    assert!(reader.at_end());
    let mut field_reader = field.payload.reader();
    read_custom_delta_header(&mut field_reader, profile.engine_network_version, 1).unwrap();
    let delta = read_fast_array_delta(
        &mut field_reader,
        &FastArrayItemLayout::Delta(vec![RepCommand::StatPairRepState]),
        &rep_codec,
        FAST_ARRAY_COUNT_CAP,
    )
    .unwrap();
    assert_eq!(
        (delta.array_replication_key, delta.base_replication_key),
        (60, 58)
    );
    let items: Vec<(i32, RepValue)> = delta
        .changed
        .iter()
        .map(|(id, item)| match item {
            FastArrayItem::Delta(stream) => (*id, stream.entries[0].value.clone()),
            FastArrayItem::Whole(_) => panic!("delta items"),
        })
        .collect();
    assert_eq!(
        items,
        [
            (1, RepValue::StatPairRepState(pair(0, 614))),
            (59, RepValue::StatPairRepState(pair(46, 53))),
        ]
    );
}
