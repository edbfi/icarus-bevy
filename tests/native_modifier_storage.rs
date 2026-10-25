//! Independently authored allocation-boundary fixtures; no original inputs.
use std::sync::Arc;

use icarus_bevy::{
    native_lifecycle::{
        CompiledNativeLifecycle, LifecycleDefinition, OwnerHandle, Presence, RecorderKind,
        SlotDeclaration, SpawnComponent, prepare_lifecycle_world,
    },
    native_modifiers::*,
    native_world::{CompiledNativeCatalog, prepare_authored_world},
    persistence::SnapshotStore,
    simulation::{DomainId, WorldSnapshot},
};

const OWNER_COUNT: usize = 1_024;

fn long_name(prefix: char, index: usize) -> String {
    let mut name = prefix.to_string().repeat(120);
    name.replace_range(..3, &format!("{index:03}"));
    name
}

fn near_budget_modifiers() -> ModifierWorldSnapshot {
    let contributions: Vec<_> = (0..9)
        .map(|index| FlatStatContribution {
            stat: long_name('s', index),
            amount: 1,
        })
        .collect();
    let entries: Vec<_> = (1..=4)
        .map(|id| ModifierEntry {
            id: ModifierId::new(id).unwrap(),
            definition: ModifierDefinition {
                name: long_name('m', id as usize),
                duration: ModifierDuration::Indefinite,
                contributions: contributions.clone(),
            },
            timer: ModifierTimer::Indefinite,
        })
        .collect();
    let cache: Vec<_> = contributions
        .into_iter()
        .map(|mut value| {
            value.amount = 4;
            value
        })
        .collect();
    // Construct current state directly: no repeated world-wide ECS preflight.
    // Each owner has five 9-element stat lists. Growing those to capacity 16
    // adds 1,146,880 charged bytes on 64-bit Linux, crossing the 8 MiB budget.
    let world = ModifierWorldSnapshot {
        format: NATIVE_MODIFIERS_FORMAT,
        owners: (1..=OWNER_COUNT)
            .map(|id| OwnerModifierState {
                format: NATIVE_MODIFIERS_FORMAT,
                owner: OwnerHandle {
                    id: DomainId(id as u64),
                    incarnation: 1,
                },
                revision: 4,
                next_id: 5,
                entries: entries.clone(),
                cache: cache.clone(),
            })
            .collect::<Vec<_>>()
            .into_boxed_slice()
            .into_vec(),
    };
    world.validate_structure().unwrap();
    world
}

fn near_budget_snapshot() -> WorldSnapshot {
    let example =
        CompiledNativeCatalog::parse(include_str!("../config/native-world.example.toml")).unwrap();
    let mut definition = example.definition().clone();
    let placement = definition.maps[0].placements[0].clone();
    definition.maps[0].placements = (0..OWNER_COUNT)
        .map(|index| {
            let mut placement = placement.clone();
            placement.key = format!("slot_{index:04}");
            placement
        })
        .collect();
    let catalog = Arc::new(CompiledNativeCatalog::compile(definition).unwrap());
    let native = prepare_authored_world(Arc::clone(&catalog), "workshop").unwrap();
    let lifecycle = CompiledNativeLifecycle::compile(
        LifecycleDefinition {
            lifecycle_format: 1,
            native_catalog_id: catalog.identity().into(),
            map: "workshop".into(),
            slots: native
                .snapshot()
                .actors
                .iter()
                .map(|actor| SlotDeclaration {
                    placement: actor.placement.clone(),
                    owner_name: actor.placement.encode_utf16().collect(),
                    recorder_kind: RecorderKind::SpatialStateV1,
                    initial_presence: Presence::Live,
                    initial_component: None,
                    spawn_component: SpawnComponent::DeclaredKind,
                })
                .collect(),
        },
        &catalog,
    )
    .unwrap();
    let mut snapshot = enable_modifier_owners(
        prepare_lifecycle_world(native.initial_snapshot(), &lifecycle)
            .unwrap()
            .into_snapshot(),
    )
    .unwrap();
    snapshot.native_modifiers = Some(near_budget_modifiers());
    snapshot.validate().unwrap();
    snapshot
}

#[test]
fn admitted_near_budget_modifier_world_survives_json_roundtrip() {
    let world = near_budget_modifiers();
    let encoded = serde_json::to_vec(&world).unwrap();
    let decoded: ModifierWorldSnapshot = serde_json::from_slice(&encoded)
        .expect("an admitted modifier world must remain within budget after decoding");
    decoded.validate_structure().unwrap();
    assert_eq!(decoded, world);
    assert_eq!(decoded.owners.capacity(), decoded.owners.len());
    for owner in &decoded.owners {
        assert_eq!(owner.entries.capacity(), owner.entries.len());
        assert_eq!(owner.cache.capacity(), owner.cache.len());
        for entry in &owner.entries {
            let contributions = &entry.definition.contributions;
            assert_eq!(contributions.capacity(), contributions.len());
        }
    }
}

#[test]
fn acknowledged_near_budget_generations_reload_without_recovery_or_rollback() {
    let directory = tempfile::tempdir().unwrap();
    let mut snapshot = near_budget_snapshot();
    snapshot.tick = 10;
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    let first = store.save_new(&snapshot).unwrap();
    snapshot.tick = 20;
    let latest = store.save(&snapshot).unwrap();
    assert_eq!(latest.generation, first.generation + 1);
    assert!(
        directory
            .path()
            .join(format!("snapshot-{}.json", first.generation))
            .is_file()
    );
    // Both retained generations contain the near-budget modifier state. A fresh
    // store must decode it rather than trusting publication's validation cache.
    drop(store);
    let store = SnapshotStore::open(directory.path()).unwrap();
    let loaded = store
        .load()
        .expect("acknowledged near-budget generations must reload")
        .unwrap();
    assert!(!loaded.recovered_previous);
    assert_eq!(loaded.snapshot, snapshot);
}
