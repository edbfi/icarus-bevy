use std::{fs, sync::Arc};

use icarus_bevy::{
    native_world::{
        Aabb3, CompiledNativeCatalog, MAX_NATIVE_CATALOG_BYTES, MAX_NATIVE_CLASSES,
        MAX_NATIVE_KEY_BYTES, MAX_NATIVE_MAPS, MAX_NATIVE_PLACEMENTS, MAX_NATIVE_STRING_UNITS,
        MAX_NATIVE_TILES, MAX_NATIVE_VARIABLES_PER_CLASS, NativeCatalogDefinition,
        NativeClassDefinition, NativeClassKind, NativeFrame, NativeMapDefinition, NativePlacement,
        NativeTileDefinition, NativeTransform, NativeValue, NativeVariableValue,
        NativeWorldSnapshot, VariableDefinition, prepare_authored_world,
    },
    simulation::DomainId,
};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const TOML_FIXTURE: &str = r#"
native_format = 1
frame = "right_handed_z_up_meters"
[[classes]]
key = "box"
kind = "spatial_object_v1"
local_bounds = { min = [-0.5, -0.5, -0.5], max = [0.5, 0.5, 0.5] }
variables = []
[[maps]]
key = "demo"
[[maps.tiles]]
key = "tile"
bounds = { min = [-10.0, -10.0, -10.0], max = [10.0, 10.0, 10.0] }
[[maps.placements]]
key = "box1"
class = "box"
tile = "tile"
transform = { translation = [0.0, 0.0, 0.0], rotation = [0.0, 0.0, 0.0, 1.0], scale = [1.0, 1.0, 1.0] }
"#;

fn variable(key: &str, default: NativeValue) -> VariableDefinition {
    VariableDefinition {
        key: key.into(),
        default,
    }
}

fn definition() -> NativeCatalogDefinition {
    let class = NativeClassDefinition {
        key: "authored_box".into(),
        kind: NativeClassKind::SpatialObjectV1,
        local_bounds: Aabb3 {
            min: [-1.0, -2.0, -0.5],
            max: [1.0, 2.0, 0.5],
        },
        variables: vec![
            variable("visible", NativeValue::Bool(true)),
            variable("health", NativeValue::I32(100)),
            variable("mass", NativeValue::F32(2.5)),
            variable("label", NativeValue::String("authored_value".into())),
        ],
    };
    let placement = |key: &str, x: f32| NativePlacement {
        key: key.into(),
        class: class.key.clone(),
        tile: "tile_a".into(),
        transform: NativeTransform {
            translation: [x, 0.0, 0.0],
            ..Default::default()
        },
        initial_values: vec![
            NativeVariableValue {
                key: "label".into(),
                value: NativeValue::String("override_value".into()),
            },
            NativeVariableValue {
                key: "health".into(),
                value: NativeValue::I32(75),
            },
        ],
    };
    let map = NativeMapDefinition {
        key: "authored_map".into(),
        tiles: vec![
            NativeTileDefinition {
                key: "tile_a".into(),
                bounds: Aabb3 {
                    min: [-50.0; 3],
                    max: [50.0; 3],
                },
            },
            NativeTileDefinition {
                key: "tile_b".into(),
                bounds: Aabb3 {
                    min: [50.0; 3],
                    max: [100.0; 3],
                },
            },
        ],
        placements: vec![placement("second", 5.0), placement("first", -5.0)],
    };
    let mut second_class = class.clone();
    second_class.key = "unused_class".into();
    let mut second_map = map.clone();
    second_map.key = "unused_map".into();
    NativeCatalogDefinition {
        native_format: 1,
        frame: NativeFrame::RightHandedZUpMeters,
        classes: vec![class, second_class],
        maps: vec![map, second_map],
    }
}

fn prepared() -> NativeWorldSnapshot {
    prepare_authored_world(
        Arc::new(CompiledNativeCatalog::compile(definition()).unwrap()),
        "authored_map",
    )
    .unwrap()
    .snapshot()
    .clone()
}

fn prepared_placement_count(count: usize) -> NativeWorldSnapshot {
    let mut authored = definition();
    authored.classes.truncate(1);
    authored.classes[0].variables.clear();
    authored.maps.truncate(1);
    let mut placement = authored.maps[0].placements[0].clone();
    placement.initial_values.clear();
    authored.maps[0].placements = (0..count)
        .map(|rank| {
            let mut placement = placement.clone();
            placement.key = format!("p{:05}", rank * 3 + 7);
            placement
        })
        .collect();
    prepare_authored_world(
        Arc::new(CompiledNativeCatalog::compile(authored).unwrap()),
        "authored_map",
    )
    .unwrap()
    .snapshot()
    .clone()
}

fn assert_snapshot_error(snapshot: &NativeWorldSnapshot, expected: &str) {
    let before = snapshot.clone();
    let error = snapshot.validate().unwrap_err();
    assert_eq!(format!("{error:#}"), expected);
    assert_eq!(*snapshot, before);
}

#[test]
fn canonical_identity_ignores_all_declaration_order_and_text_formatting() {
    let original = CompiledNativeCatalog::compile(definition()).unwrap();
    let mut reordered = definition();
    reordered.classes.reverse();
    reordered.maps.reverse();
    for class in &mut reordered.classes {
        class.variables.reverse();
    }
    for map in &mut reordered.maps {
        map.tiles.reverse();
        map.placements.reverse();
        for placement in &mut map.placements {
            placement.initial_values.reverse();
        }
    }
    let json = serde_json::to_string_pretty(&reordered).unwrap();
    let parsed = CompiledNativeCatalog::parse(&json).unwrap();
    assert_eq!(original.identity(), parsed.identity());
    assert_eq!(original.definition(), parsed.definition());
    let digest = Sha256::digest(serde_json::to_vec(original.definition()).unwrap());
    let expected: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
    assert_eq!(original.identity(), expected);
    let world = prepare_authored_world(Arc::new(parsed), "authored_map").unwrap();
    let snapshot = world.snapshot();
    assert_eq!(snapshot.actors.len(), 2);
    assert_eq!(snapshot.actors[0].id, DomainId(1));
    assert_eq!(snapshot.actors[0].placement, "first");
    assert_eq!(snapshot.actors[1].id, DomainId(2));
    let variables = &snapshot.actors[0].variables;
    assert_eq!(
        variables.iter().map(|v| v.key.as_str()).collect::<Vec<_>>(),
        ["health", "label", "mass", "visible"]
    );
    assert_eq!(variables[0].value, NativeValue::I32(75));
    assert_eq!(
        variables[1].value,
        NativeValue::String("override_value".into())
    );
    assert_eq!(variables[2].value, NativeValue::F32(2.5));
    assert_eq!(variables[3].value, NativeValue::Bool(true));
    snapshot.validate().unwrap();
    let encoded = serde_json::to_string(snapshot).unwrap();
    let roundtrip: NativeWorldSnapshot = serde_json::from_str(&encoded).unwrap();
    assert_eq!(*snapshot, roundtrip);
}

#[test]
fn authored_toml_load_and_json_share_the_contract() {
    let catalog = CompiledNativeCatalog::parse(TOML_FIXTURE).unwrap();
    let world = prepare_authored_world(Arc::new(catalog.clone()), "demo").unwrap();
    assert_eq!(world.snapshot().actors[0].id, DomainId(1));
    assert!(world.snapshot().actors[0].variables.is_empty());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.toml");
    fs::write(&path, TOML_FIXTURE).unwrap();
    assert_eq!(
        CompiledNativeCatalog::load(&path).unwrap().identity(),
        catalog.identity()
    );
    assert!(CompiledNativeCatalog::load(directory.path()).is_err());
    let json = serde_json::to_string(catalog.definition()).unwrap();
    assert_eq!(
        CompiledNativeCatalog::parse(&json).unwrap().identity(),
        catalog.identity()
    );
}

#[test]
fn nonidentity_rotation_scale_translation_and_bounds_use_all_corners() {
    let local = Aabb3 {
        min: [-1.0, -2.0, -3.0],
        max: [1.0, 2.0, 3.0],
    };
    let transform = NativeTransform {
        translation: [10.0, 20.0, 30.0],
        rotation: [
            0.0,
            0.0,
            std::f32::consts::FRAC_1_SQRT_2,
            std::f32::consts::FRAC_1_SQRT_2,
        ],
        scale: [2.0, 3.0, 4.0],
    };
    let bounds = transform.transformed_bounds(&local).unwrap();
    for (actual, expected) in bounds.min.into_iter().zip([4.0, 18.0, 18.0]) {
        assert!((actual - expected).abs() < 0.00001);
    }
    for (actual, expected) in bounds.max.into_iter().zip([16.0, 22.0, 42.0]) {
        assert!((actual - expected).abs() < 0.00001);
    }
    let mut authored = definition();
    authored.maps[0].placements[0].transform = transform;
    CompiledNativeCatalog::compile(authored).unwrap();
    assert_eq!(
        NativeTransform::default()
            .transformed_bounds(&local)
            .unwrap(),
        local
    );
}

#[test]
fn transforms_bounds_and_overflow_fail_closed() {
    let local = Aabb3 {
        min: [-1.0; 3],
        max: [1.0; 3],
    };
    for transform in [
        NativeTransform {
            rotation: [0.0; 4],
            ..Default::default()
        },
        NativeTransform {
            rotation: [0.0, 0.0, 0.0, 1.001],
            ..Default::default()
        },
        NativeTransform {
            rotation: [f32::NAN, 0.0, 0.0, 1.0],
            ..Default::default()
        },
        NativeTransform {
            scale: [1.0, 0.0, 1.0],
            ..Default::default()
        },
        NativeTransform {
            scale: [-1.0, 1.0, 1.0],
            ..Default::default()
        },
        NativeTransform {
            translation: [f32::INFINITY, 0.0, 0.0],
            ..Default::default()
        },
    ] {
        assert!(transform.transformed_bounds(&local).is_err());
    }
    let huge = NativeTransform {
        translation: [f32::MAX; 3],
        scale: [f32::MAX; 3],
        ..Default::default()
    };
    assert!(huge.transformed_bounds(&local).is_err());
    for bounds in [
        Aabb3 {
            min: [1.0; 3],
            max: [-1.0; 3],
        },
        Aabb3 {
            min: [f32::NEG_INFINITY; 3],
            max: [1.0; 3],
        },
        Aabb3 {
            min: [-1.0; 3],
            max: [f32::NAN; 3],
        },
    ] {
        assert!(
            NativeTransform::default()
                .transformed_bounds(&bounds)
                .is_err()
        );
    }
    NativeTransform {
        rotation: [0.0, 0.0, 0.0, 1.000001],
        ..Default::default()
    }
    .validate()
    .unwrap();
}

#[test]
fn any_invalid_placement_prevents_a_catalog_even_on_an_unselected_map() {
    for invalid_map in [0, 1] {
        let mut authored = definition();
        authored.maps[invalid_map].placements[1]
            .transform
            .translation = [1000.0; 3];
        assert!(CompiledNativeCatalog::compile(authored).is_err());
    }
    let catalog = Arc::new(CompiledNativeCatalog::compile(definition()).unwrap());
    assert!(prepare_authored_world(catalog.clone(), "missing_map").is_err());
    let first = prepare_authored_world(catalog.clone(), "authored_map").unwrap();
    assert_eq!(
        first.snapshot(),
        prepare_authored_world(catalog, "authored_map")
            .unwrap()
            .snapshot()
    );
}

#[test]
fn duplicate_keys_are_rejected_in_every_scope_before_sorting() {
    let mutations: Vec<fn(&mut NativeCatalogDefinition)> = vec![
        |d| d.classes.push(d.classes[0].clone()),
        |d| d.maps.push(d.maps[0].clone()),
        |d| {
            let duplicate = d.classes[0].variables[0].clone();
            d.classes[0].variables.push(duplicate);
        },
        |d| {
            let duplicate = d.maps[0].tiles[0].clone();
            d.maps[0].tiles.push(duplicate);
        },
        |d| {
            let duplicate = d.maps[0].placements[0].clone();
            d.maps[0].placements.push(duplicate);
        },
        |d| {
            let duplicate = d.maps[0].placements[0].initial_values[0].clone();
            d.maps[0].placements[0].initial_values.push(duplicate);
        },
    ];
    for mutate in mutations {
        let mut authored = definition();
        mutate(&mut authored);
        assert!(CompiledNativeCatalog::compile(authored).is_err());
    }
}

#[test]
fn dangling_cross_map_and_wrong_type_bindings_are_rejected() {
    let mutations: Vec<fn(&mut NativeCatalogDefinition)> = vec![
        |d| d.native_format = 2,
        |d| d.maps[0].placements[0].class = "absent".into(),
        |d| d.maps[0].placements[0].tile = "absent".into(),
        |d| d.maps[0].placements[0].initial_values[0].key = "absent".into(),
        |d| d.maps[0].placements[0].initial_values[0].value = NativeValue::I32(3),
        |d| d.classes[0].variables[0].default = NativeValue::F32(f32::NAN),
        |d| d.maps[0].placements[0].initial_values[1].value = NativeValue::F32(f32::INFINITY),
        |d| d.maps[0].placements.clear(),
        |d| d.maps[0].tiles[0].bounds.min = [f32::NAN; 3],
        |d| d.classes[0].local_bounds.max = [-5.0; 3],
    ];
    for mutate in mutations {
        let mut authored = definition();
        mutate(&mut authored);
        assert!(CompiledNativeCatalog::compile(authored).is_err());
    }
    let mut authored = definition();
    authored.maps[1].tiles[0].key = "other_map_only".into();
    for p in &mut authored.maps[1].placements {
        p.tile = "other_map_only".into();
    }
    authored.maps[0].placements[0].tile = "other_map_only".into();
    assert!(CompiledNativeCatalog::compile(authored).is_err());
}

#[test]
fn snapshot_requires_full_topology_stable_ids_exact_bindings_and_schema() {
    let mutations: Vec<fn(&mut NativeWorldSnapshot)> = vec![
        |s| s.actors.clear(),
        |s| {
            s.actors.pop();
        },
        |s| s.actors.push(s.actors[0].clone()),
        |s| s.actors[1] = s.actors[0].clone(),
        |s| s.actors[0].placement = "extra".into(),
        |s| s.actors[0].id = DomainId(0),
        |s| s.actors[0].id = DomainId(2),
        |s| s.actors[0].class = "unused_class".into(),
        |s| s.actors[0].tile = "tile_b".into(),
        |s| s.map = "absent".into(),
        |s| s.actors[0].variables.clear(),
        |s| {
            s.actors[0].variables.pop();
        },
        |s| s.actors[0].variables[1] = s.actors[0].variables[0].clone(),
        |s| s.actors[0].variables[0].key = "extra".into(),
        |s| s.actors[0].variables[0].value = NativeValue::Bool(true),
        |s| s.actors[0].transform.translation = [1000.0; 3],
        |s| s.actors[0].transform.scale = [0.0; 3],
        |s| s.catalog_id = "0".repeat(64),
        |s| s.definitions.classes[0].variables[0].default = NativeValue::I32(1),
    ];
    for mutate in mutations {
        let mut snapshot = prepared();
        mutate(&mut snapshot);
        assert!(snapshot.validate().is_err());
    }
    let mut snapshot = prepared();
    snapshot.actors.reverse();
    for actor in &mut snapshot.actors {
        actor.variables.reverse();
        actor.transform.translation[2] = 10.0;
        actor
            .variables
            .iter_mut()
            .find(|v| v.key == "health")
            .unwrap()
            .value = NativeValue::I32(20);
    }
    snapshot.validate().unwrap();
}

#[test]
fn snapshot_accepts_arbitrary_placement_order_through_the_placement_cap() {
    for count in [1, 64, 1024, MAX_NATIVE_PLACEMENTS] {
        let canonical = prepared_placement_count(count);
        assert_eq!(canonical.actors.last().unwrap().id, DomainId(count as u64));
        let mut snapshot = canonical.clone();
        for order in 0..3 {
            match order {
                0 => {}
                1 => snapshot.actors.reverse(),
                _ => {
                    // Visit separated ranks first, retaining each ID/key binding.
                    snapshot.actors = (1..count)
                        .step_by(2)
                        .chain((0..count).step_by(2))
                        .map(|index| canonical.actors[index].clone())
                        .collect();
                }
            }
            let before = snapshot.clone();
            snapshot.validate().unwrap();
            assert_eq!(snapshot, before);
        }
        if count == MAX_NATIVE_PLACEMENTS {
            snapshot.actors.pop();
            assert_snapshot_error(&snapshot, "native snapshot placement set incomplete");
            snapshot.actors = canonical.actors.clone();
            snapshot.actors.push(canonical.actors[0].clone());
            assert_snapshot_error(&snapshot, "native snapshot placement set incomplete");
            let mut oversized = canonical.definitions.clone();
            let mut extra = oversized.maps[0].placements[0].clone();
            extra.key = "extra".into();
            oversized.maps[0].placements.push(extra);
            assert_eq!(
                format!(
                    "{:#}",
                    CompiledNativeCatalog::compile(oversized).unwrap_err()
                ),
                "native map index 0: native array limit"
            );
        }
    }
}

#[test]
fn snapshot_duplicate_errors_use_input_indices_for_first_middle_and_last_ranks() {
    let clean = prepared_placement_count(7);
    for reverse in [false, true] {
        for rank in [0, 3, 6] {
            let mut snapshot = clean.clone();
            if reverse {
                snapshot.actors.reverse();
            }
            let source_index = snapshot
                .actors
                .iter()
                .position(|actor| actor.id == DomainId(rank + 1))
                .unwrap();
            let replaced_index = (source_index + 3) % snapshot.actors.len();
            snapshot.actors[replaced_index] = snapshot.actors[source_index].clone();
            assert_snapshot_error(
                &snapshot,
                &format!(
                    "native duplicate placement index {}",
                    source_index.max(replaced_index)
                ),
            );
        }
    }
}

#[test]
fn snapshot_validates_duplicate_actors_before_checking_occupancy_on_every_call() {
    let catalog = Arc::new(CompiledNativeCatalog::compile(definition()).unwrap());
    let clean = prepare_authored_world(catalog.clone(), "authored_map")
        .unwrap()
        .snapshot()
        .clone();
    let corruptions: Vec<fn(&mut NativeWorldSnapshot)> = vec![
        |s| s.actors[1].variables[0].value = NativeValue::Bool(true),
        |s| s.actors[1].variables.clear(),
        |s| s.actors[1].transform.rotation = [0.0; 4],
        |s| s.actors[1].transform.translation = [1000.0; 3],
        |s| s.actors[1].id = DomainId(0),
        |s| s.actors[1].id = DomainId(u64::MAX),
    ];
    let reasons = [
        "native variable type index 0",
        "native variable schema incomplete",
        "native transform quaternion is not unit length",
        "native actor outside tile",
        "native actor domain identity mismatch",
        "native actor domain identity mismatch",
    ];
    for (corrupt, reason) in corruptions.into_iter().zip(reasons) {
        let mut snapshot = clean.clone();
        snapshot.actors[1] = snapshot.actors[0].clone();
        corrupt(&mut snapshot);
        for _ in 0..2 {
            assert_snapshot_error(&snapshot, &format!("native actor index 1: {reason}"));
        }
        // Repair only the actor defect: the equal-count duplicate still fails.
        snapshot.actors[1] = clean.actors[0].clone();
        for _ in 0..2 {
            assert_snapshot_error(&snapshot, "native duplicate placement index 1");
        }
        snapshot.actors[1] = clean.actors[1].clone();
        for _ in 0..2 {
            snapshot.validate().unwrap();
        }
        assert_eq!(snapshot, clean);
        let world = prepare_authored_world(catalog.clone(), "authored_map").unwrap();
        assert_eq!(*world.snapshot(), clean);
    }
}

#[test]
fn snapshot_reports_the_first_duplicate_or_corrupt_actor_in_input_order() {
    let clean = prepared_placement_count(3);
    let mut snapshot = clean.clone();
    snapshot.actors[1] = snapshot.actors[0].clone();
    snapshot.actors[2].transform.translation = [1000.0; 3];
    assert_snapshot_error(&snapshot, "native duplicate placement index 1");
    snapshot.actors.swap(1, 2);
    assert_snapshot_error(&snapshot, "native actor index 1: native actor outside tile");
    snapshot.actors[1] = clean.actors[2].clone();
    assert_snapshot_error(&snapshot, "native duplicate placement index 2");
    snapshot.actors[2] = clean.actors[1].clone();
    snapshot.validate().unwrap();
}

#[test]
fn snapshot_map_count_and_placement_errors_precede_actor_defects() {
    let clean = prepared();
    let mut snapshot = clean.clone();
    snapshot.actors.clear();
    snapshot.map = "private invalid/map".into();
    assert_snapshot_error(&snapshot, "native snapshot map: native identifier invalid");
    snapshot.map = "private_missing_map".into();
    assert_snapshot_error(&snapshot, "native snapshot map binding");
    snapshot.map = clean.map.clone();
    assert_snapshot_error(&snapshot, "native snapshot placement set incomplete");
    for count in [1, 3] {
        snapshot.actors = vec![clean.actors[0].clone(); count];
        snapshot.actors[0].placement = "private invalid/placement".into();
        assert_snapshot_error(&snapshot, "native snapshot placement set incomplete");
    }
    snapshot = clean.clone();
    snapshot.actors[0].placement = "private invalid/placement".into();
    snapshot.actors[0].id = DomainId(u64::MAX);
    snapshot.actors[0].variables[0].value = NativeValue::Bool(true);
    snapshot.actors[0].transform.rotation = [0.0; 4];
    snapshot.actors[1].id = DomainId(0);
    assert_snapshot_error(&snapshot, "native actor index 0: native identifier invalid");
    snapshot.actors[0].placement = "private_missing_placement".into();
    assert_snapshot_error(
        &snapshot,
        "native actor index 0: native actor placement binding",
    );
    snapshot.actors.reverse();
    assert_snapshot_error(
        &snapshot,
        "native actor index 0: native actor domain identity mismatch",
    );
}

#[test]
fn snapshot_rejects_extreme_and_cross_placement_ids_before_type_or_pose_errors() {
    let clean = prepared();
    for reverse in [false, true] {
        let mut ordered = clean.clone();
        if reverse {
            ordered.actors.reverse();
        }
        for index in 0..ordered.actors.len() {
            for id in [
                DomainId(0),
                DomainId(u64::MAX),
                ordered.actors[1 - index].id,
            ] {
                let mut snapshot = ordered.clone();
                snapshot.actors[index].id = id;
                snapshot.actors[index].variables[0].value = NativeValue::Bool(true);
                snapshot.actors[index].transform.rotation = [0.0; 4];
                assert_snapshot_error(
                    &snapshot,
                    &format!("native actor index {index}: native actor domain identity mismatch"),
                );
            }
        }
        ordered.validate().unwrap();
    }
}

#[test]
fn snapshot_rechecks_embedded_definitions_and_identity_with_unchanged_records() {
    let mut snapshot = prepared();
    // A valid change to an unused definition must still invalidate catalog_id.
    snapshot.definitions.classes[1].local_bounds.min[0] -= 0.25;
    assert!(snapshot.validate().is_err());
    snapshot.catalog_id = CompiledNativeCatalog::compile(snapshot.definitions.clone())
        .unwrap()
        .identity()
        .to_owned();
    snapshot.validate().unwrap();
    // Recompilation must include definitions outside the active map as well.
    snapshot.definitions.maps[1].placements[0].transform.scale = [0.0; 3];
    assert!(snapshot.validate().is_err());
}

#[test]
fn keys_and_utf16_values_obey_exact_limits() {
    for key in [
        "".to_owned(),
        "x".repeat(MAX_NATIVE_KEY_BYTES + 1),
        "bad key".into(),
        "é".into(),
        "/asset/path".into(),
    ] {
        let mut authored = definition();
        authored.maps[0].key = key;
        assert!(CompiledNativeCatalog::compile(authored).is_err());
    }
    let mut authored = definition();
    authored.maps[0].key = format!("a-_.{}", "Z".repeat(MAX_NATIVE_KEY_BYTES - 4));
    authored.classes[0].variables[0].default =
        NativeValue::String("😀".repeat(MAX_NATIVE_STRING_UNITS / 2));
    CompiledNativeCatalog::compile(authored.clone()).unwrap();
    authored.classes[0].variables[0].default =
        NativeValue::String("😀".repeat(MAX_NATIVE_STRING_UNITS / 2 + 1));
    assert!(CompiledNativeCatalog::compile(authored).is_err());
    let mut snapshot = prepared();
    snapshot.actors[0].variables[1].value =
        NativeValue::String("x".repeat(MAX_NATIVE_STRING_UNITS + 1));
    assert!(snapshot.validate().is_err());
}

#[test]
fn array_limits_and_aggregate_map_limits_apply_to_programmatic_compilation() {
    let mut authored = definition();
    authored.classes = (0..=MAX_NATIVE_CLASSES)
        .map(|i| {
            let mut c = authored.classes[0].clone();
            c.key = format!("c{i}");
            c
        })
        .collect();
    assert!(CompiledNativeCatalog::compile(authored).is_err());
    let mut authored = definition();
    authored.maps = (0..=MAX_NATIVE_MAPS)
        .map(|i| {
            let mut m = authored.maps[0].clone();
            m.key = format!("m{i}");
            m
        })
        .collect();
    assert!(CompiledNativeCatalog::compile(authored).is_err());
    let mut authored = definition();
    authored.classes[0].variables = (0..=MAX_NATIVE_VARIABLES_PER_CLASS)
        .map(|i| variable(&format!("v{i}"), NativeValue::Bool(true)))
        .collect();
    assert!(CompiledNativeCatalog::compile(authored).is_err());
    for (count, tile_limit) in [
        (MAX_NATIVE_TILES / 2 + 1, true),
        (MAX_NATIVE_PLACEMENTS / 2 + 1, false),
    ] {
        let mut authored = definition();
        for map in &mut authored.maps {
            if tile_limit {
                let tile = map.tiles[0].clone();
                map.tiles = (0..count)
                    .map(|i| {
                        let mut t = tile.clone();
                        t.key = format!("tile_{i}");
                        t
                    })
                    .collect();
                for p in &mut map.placements {
                    p.tile = "tile_0".into();
                }
            } else {
                let placement = map.placements[0].clone();
                map.placements = (0..count)
                    .map(|i| {
                        let mut p = placement.clone();
                        p.key = format!("p{i}");
                        p
                    })
                    .collect();
            }
        }
        assert!(CompiledNativeCatalog::compile(authored).is_err());
    }
}

#[test]
fn byte_limit_applies_to_text_files_and_programmatic_serialization() {
    let oversized = " ".repeat(MAX_NATIVE_CATALOG_BYTES + 1);
    assert!(CompiledNativeCatalog::parse(&oversized).is_err());
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("large");
    fs::write(&path, oversized).unwrap();
    assert!(CompiledNativeCatalog::load(&path).is_err());
    let mut authored = definition();
    authored.maps.truncate(1);
    authored.classes[0].variables = (0..64)
        .map(|i| variable(&format!("v{i}"), NativeValue::String("x".repeat(1024))))
        .collect();
    let template = authored.maps[0].placements[0].clone();
    authored.maps[0].placements = (0..80)
        .map(|i| NativePlacement {
            key: format!("p{i}"),
            initial_values: authored.classes[0]
                .variables
                .iter()
                .map(|v| NativeVariableValue {
                    key: v.key.clone(),
                    value: v.default.clone(),
                })
                .collect(),
            ..template.clone()
        })
        .collect();
    assert!(CompiledNativeCatalog::compile(authored).is_err());
}

fn replace_path(value: &mut Value, path: &[&str], replacement: Value) {
    let mut current = value;
    for part in &path[..path.len() - 1] {
        current = if let Ok(index) = part.parse::<usize>() {
            &mut current[index]
        } else {
            &mut current[*part]
        };
    }
    let last = path[path.len() - 1];
    if let Ok(index) = last.parse::<usize>() {
        current[index] = replacement;
    } else {
        current[last] = replacement;
    }
}

#[test]
fn every_named_object_rejects_positional_arrays_and_unknown_fields() {
    let snapshot = serde_json::to_value(prepared()).unwrap();
    // Use declaration-order arrays that an ordinary derived struct would
    // accept, rather than malformed arrays that already fail on field types.
    let paths: &[(&[&str], &[&str])] = &[
        (
            &["definitions"],
            &["native_format", "frame", "classes", "maps"],
        ),
        (
            &["definitions", "classes", "0"],
            &["key", "kind", "local_bounds", "variables"],
        ),
        (
            &["definitions", "classes", "0", "local_bounds"],
            &["min", "max"],
        ),
        (
            &["definitions", "classes", "0", "variables", "0"],
            &["key", "default"],
        ),
        (
            &["definitions", "classes", "0", "variables", "0", "default"],
            &["type", "value"],
        ),
        (
            &["definitions", "maps", "0"],
            &["key", "tiles", "placements"],
        ),
        (
            &["definitions", "maps", "0", "tiles", "0"],
            &["key", "bounds"],
        ),
        (
            &["definitions", "maps", "0", "tiles", "0", "bounds"],
            &["min", "max"],
        ),
        (
            &["definitions", "maps", "0", "placements", "0"],
            &["key", "class", "tile", "transform", "initial_values"],
        ),
        (
            &["definitions", "maps", "0", "placements", "0", "transform"],
            &["translation", "rotation", "scale"],
        ),
        (
            &[
                "definitions",
                "maps",
                "0",
                "placements",
                "0",
                "initial_values",
                "0",
            ],
            &["key", "value"],
        ),
        (
            &["actors", "0"],
            &["id", "placement", "class", "tile", "transform", "variables"],
        ),
        (
            &["actors", "0", "transform"],
            &["translation", "rotation", "scale"],
        ),
        (&["actors", "0", "variables", "0"], &["key", "value"]),
        (
            &["actors", "0", "variables", "0", "value"],
            &["type", "value"],
        ),
    ];
    let positional_snapshot = json!([
        snapshot["catalog_id"],
        snapshot["definitions"],
        snapshot["map"],
        snapshot["actors"]
    ]);
    assert!(serde_json::from_value::<NativeWorldSnapshot>(positional_snapshot).is_err());
    let d = &snapshot["definitions"];
    assert!(
        serde_json::from_value::<NativeCatalogDefinition>(json!([
            d["native_format"],
            d["frame"],
            d["classes"],
            d["maps"]
        ]))
        .is_err()
    );
    for (path, field_order) in paths {
        let mut current = &snapshot;
        for part in *path {
            current = if let Ok(index) = part.parse::<usize>() {
                &current[index]
            } else {
                &current[*part]
            };
        }
        let mut array = snapshot.clone();
        let fields = current.as_object().unwrap();
        replace_path(
            &mut array,
            path,
            Value::Array(field_order.iter().map(|key| fields[*key].clone()).collect()),
        );
        assert!(
            serde_json::from_value::<NativeWorldSnapshot>(array).is_err(),
            "{path:?}"
        );
        let mut unknown = snapshot.clone();
        let mut fields = fields.clone();
        fields.insert("unexpected".into(), json!(true));
        replace_path(&mut unknown, path, Value::Object(fields));
        assert!(
            serde_json::from_value::<NativeWorldSnapshot>(unknown).is_err(),
            "{path:?}"
        );
    }
    let mut unknown = snapshot;
    unknown["unexpected"] = json!(true);
    assert!(serde_json::from_value::<NativeWorldSnapshot>(unknown).is_err());
    assert!(
        CompiledNativeCatalog::parse(&TOML_FIXTURE.replace("native_format", "format")).is_err()
    );
    assert!(
        CompiledNativeCatalog::parse(
            &TOML_FIXTURE.replace("native_format = 1", "native_format = 1\nunexpected = true")
        )
        .is_err()
    );
}

#[test]
fn debug_and_error_chains_never_include_authored_names_values_or_paths() {
    let catalog = Arc::new(CompiledNativeCatalog::compile(definition()).unwrap());
    let world = prepare_authored_world(catalog.clone(), "authored_map").unwrap();
    let snapshot = world.snapshot();
    let debug = format!(
        "{catalog:?} {world:?} {snapshot:?} {:?} {:?} {:?} {:?}",
        catalog.definition(),
        snapshot.actors,
        catalog.definition().maps,
        catalog.definition().classes
    );
    for private in [
        "authored_map",
        "authored_box",
        "authored_value",
        "override_value",
        "tile_a",
        "first",
        catalog.identity(),
    ] {
        assert!(!debug.contains(private));
    }
    for text in [
        r#"{"native_format":1,"private_unknown_key":"private_value"}"#,
        "native_format = 1\nprivate_unknown_key = 'private_value'",
    ] {
        let error = format!("{:#}", CompiledNativeCatalog::parse(text).unwrap_err());
        assert!(!error.contains("private_unknown_key"));
        assert!(!error.contains("private_value"));
    }
    let mut invalid = definition();
    invalid.maps[0].placements[0].class = "private_missing_class".into();
    let error = format!("{:#}", CompiledNativeCatalog::compile(invalid).unwrap_err());
    assert!(!error.contains("private_missing_class"));
    let error = format!(
        "{:#}",
        prepare_authored_world(catalog, "private_missing_map").unwrap_err()
    );
    assert!(!error.contains("private_missing_map"));
    let directory = tempfile::tempdir().unwrap();
    let error = format!(
        "{:#}",
        CompiledNativeCatalog::load(&directory.path().join("private_missing_path")).unwrap_err()
    );
    assert!(!error.contains("private_missing_path"));
}

#[test]
fn small_catalog_cannot_expand_shared_string_defaults_without_bound() {
    let mut source = definition();
    source.classes.truncate(1);
    source.classes[0].variables = (0..64)
        .map(|i| VariableDefinition {
            key: format!("v{i}"),
            default: NativeValue::String("x".repeat(1024)),
        })
        .collect();
    source.maps.truncate(1);
    let mut placement = source.maps[0].placements[0].clone();
    placement.class = source.classes[0].key.clone();
    placement.initial_values.clear();
    source.maps[0].placements = (0..600)
        .map(|i| {
            let mut value = placement.clone();
            value.key = format!("p{i}");
            value
        })
        .collect();
    assert!(serde_json::to_vec(&source).unwrap().len() < MAX_NATIVE_CATALOG_BYTES);
    assert!(CompiledNativeCatalog::compile(source).is_err());
}
