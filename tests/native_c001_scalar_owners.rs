//! Independently authored scalar source, provenance, and transaction tests.
#[path = "common/c001_scalar_owners.rs"]
mod authored;
#[path = "common/c001_owners.rs"]
mod old;
use authored::{Fixture, Tag, fixture, scalar_array, scalar_pair, stream, string, units};
use icarus_bevy::{
    import_candidate::DecodedImportCandidate,
    native_c001_scalar_owners::{
        C001ScalarArrayV1, C001ScalarLayoutV1, C001ScalarOwnerAdapter, C001ScalarOwnerBindingV1,
        C001ScalarOwnerBindingsV1, C001ScalarOwnerImportMetadataV1,
        CompiledC001ScalarOwnerBindings, ResolvedC001ScalarOwnerV1,
        prepare_c001_scalar_owner_import, publish_new_c001_scalar_owner_import,
    },
    native_lifecycle::{RestoreDisposition, prepare_owner_restore},
    native_world::{MAX_NATIVE_PLACEMENTS, NativeValue},
    simulation::{DomainId, SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS},
};
use serde_json::{Value, json};
fn rejected(f: &mut Fixture) {
    f.rebuild_source();
    if let Ok(candidate) = DecodedImportCandidate::decode(f.envelope.as_slice()) {
        assert!(
            prepare_c001_scalar_owner_import(&candidate, f.base.clone(), &f.bindings, &f.contracts)
                .is_err()
        );
    }
}
fn replace_enabled(f: &mut Fixture, tag: Tag) {
    let slot = f.owners[2]
        .iter_mut()
        .find(|t| t.name == "AuthoredEnabled")
        .unwrap();
    *slot = tag;
}
#[test]
fn complete_saved_pairs_overwrite_defaults_on_attach_spawn_and_replacement() {
    let f = fixture();
    let p = f.prepared();
    let s = p.snapshot();
    assert_eq!(s.schema_version, SNAPSHOT_SCHEMA_C001_SCALAR_OWNERS);
    assert_eq!(
        (
            p.inspection().owners.attached,
            p.inspection().owners.spawned,
            p.inspection().owners.replaced,
            p.inspection().scalar_properties
        ),
        (1, 1, 1, 6)
    );
    let native = s.native_world.as_ref().unwrap();
    for (id, enabled, count, incarnation) in [(1, false, 2, 1), (2, true, 2, 2), (3, true, 1, 1)] {
        let actor = native.actors.iter().find(|a| a.id == DomainId(id)).unwrap();
        assert_eq!(actor.variables, scalar_pair(enabled, count));
        assert_eq!(
            s.native_lifecycle.as_ref().unwrap().slots[id as usize - 1].incarnation,
            incarnation
        );
    }
    let dispositions: Vec<_> = s
        .native_scalar_owner_import
        .as_ref()
        .unwrap()
        .initial_completion
        .outcomes
        .iter()
        .map(|o| o.disposition)
        .collect();
    assert!(dispositions.contains(&RestoreDisposition::Attached));
    s.validate().unwrap();
}
#[test]
fn canonical_bindings_use_placement_order_and_retain_exact_utf16() {
    let f = fixture();
    let b = f.bindings.definition();
    assert_eq!(
        b.owners
            .iter()
            .map(|b| b.owner.native_placement.as_str())
            .collect::<Vec<_>>(),
        vec!["alpha", "middle", "zulu"]
    );
    assert_eq!(b.owners[0].enabled_source_name, vec![0xd800, 69]);
    let mut reversed = b.clone();
    reversed.owners.reverse();
    let compiled = CompiledC001ScalarOwnerBindings::compile(reversed).unwrap();
    assert_eq!(compiled.identity(), f.bindings.identity());
    let s = f.snapshot();
    let m = s.native_scalar_owner_import.as_ref().unwrap();
    assert_eq!(
        m.resolved
            .iter()
            .map(|r| r.binding_index)
            .collect::<Vec<_>>(),
        vec![2, 0, 1]
    );
    assert_eq!(m.source_prospect_id, vec![0xd800, 65]);
    for substitute in [
        vec![0xfffd, 69],
        units("éUses"),
        units("UsesE"),
        units("Usese\u{301}"),
    ] {
        let mut binding = b.clone();
        binding.owners[0].remaining_uses_source_name = substitute;
        let binding = CompiledC001ScalarOwnerBindings::compile(binding).unwrap();
        assert!(
            prepare_c001_scalar_owner_import(
                &f.candidate(),
                f.base.clone(),
                &binding,
                &f.contracts
            )
            .is_err()
        );
    }
}
#[test]
fn source_arrays_require_exact_names_types_headers_count_and_members() {
    for variant in 0..20 {
        let mut f = fixture();
        let tag = match variant {
            0 => scalar_array(
                "AuthoredEnabled",
                "wrong",
                "AuthoredBoolRecord",
                Tag::boolean("SavedBool", true),
                &[0xd800, 69],
            ),
            1 => scalar_array(
                "AuthoredEnabled",
                "EnabledElement",
                "wrong",
                Tag::boolean("SavedBool", true),
                &[0xd800, 69],
            ),
            2 => scalar_array(
                "AuthoredEnabled",
                "EnabledElement",
                "AuthoredBoolRecord",
                Tag::integer("SavedBool", 1),
                &[0xd800, 69],
            ),
            3 => scalar_array(
                "AuthoredEnabled",
                "EnabledElement",
                "AuthoredBoolRecord",
                Tag::boolean("wrong", true),
                &[0xd800, 69],
            ),
            4 => scalar_array(
                "AuthoredEnabled",
                "EnabledElement",
                "AuthoredBoolRecord",
                Tag::boolean("SavedBool", true),
                &[0xfffd, 69],
            ),
            _ => f.owners[2]
                .iter()
                .find(|t| t.name == "AuthoredEnabled")
                .unwrap()
                .clone(),
        };
        replace_enabled(&mut f, tag);
        let array = f.owners[2]
            .iter_mut()
            .find(|t| t.name == "AuthoredEnabled")
            .unwrap();
        match variant {
            5 => array.index = 1,
            6 => array.extras = [string("IntProperty"), vec![0]].concat(),
            7 => array.payload[..4].copy_from_slice(&0_i32.to_le_bytes()),
            8 => array.payload[..4].copy_from_slice(&2_i32.to_le_bytes()),
            9 => array.payload.extend([0]),
            10 => array.property_guid(),
            11 => {
                let mut value = Tag::boolean("SavedBool", true);
                value.extras[0] = 2;
                *array = scalar_array(
                    "AuthoredEnabled",
                    "EnabledElement",
                    "AuthoredBoolRecord",
                    value,
                    &[0xd800, 69],
                );
            }
            12 => {
                let mut value = Tag::boolean("SavedBool", true);
                value.payload = vec![1];
                *array = scalar_array(
                    "AuthoredEnabled",
                    "EnabledElement",
                    "AuthoredBoolRecord",
                    value,
                    &[0xd800, 69],
                );
            }
            13 => {
                *array = Tag::array(
                    "AuthoredEnabled",
                    "StructProperty",
                    [
                        1_i32.to_le_bytes().to_vec(),
                        Tag::structure(
                            "EnabledElement",
                            "AuthoredBoolRecord",
                            stream(&[
                                Tag::text("VariableName", "StrProperty", &[0xd800, 69]),
                                Tag::boolean("SavedBool", true),
                            ]),
                        )
                        .bytes(),
                    ]
                    .concat(),
                )
            }
            14 => {
                *array = Tag::array(
                    "AuthoredEnabled",
                    "StructProperty",
                    [
                        1_i32.to_le_bytes().to_vec(),
                        Tag::structure(
                            "EnabledElement",
                            "AuthoredBoolRecord",
                            stream(&[
                                Tag::text("VariableName", "NameProperty", &[0xd800, 69]),
                                Tag::boolean("SavedBool", true),
                                Tag::integer("Extra", 0),
                            ]),
                        )
                        .bytes(),
                    ]
                    .concat(),
                )
            }
            15 => {
                *array = Tag::array(
                    "AuthoredEnabled",
                    "StructProperty",
                    [
                        1_i32.to_le_bytes().to_vec(),
                        Tag::structure(
                            "EnabledElement",
                            "AuthoredBoolRecord",
                            stream(&[
                                Tag::text("VariableName", "NameProperty", &[0xd800, 69]),
                                Tag::text("VariableName", "NameProperty", &[0xd800, 69]),
                            ]),
                        )
                        .bytes(),
                    ]
                    .concat(),
                )
            }
            16 => {
                let mut value = Tag::boolean("SavedBool", true);
                value.index = 1;
                *array = scalar_array(
                    "AuthoredEnabled",
                    "EnabledElement",
                    "AuthoredBoolRecord",
                    value,
                    &[0xd800, 69],
                );
            }
            17 => {
                let mut shared = Tag::structure(
                    "EnabledElement",
                    "AuthoredBoolRecord",
                    stream(&[
                        Tag::text("VariableName", "NameProperty", &[0xd800, 69]),
                        Tag::boolean("SavedBool", true),
                    ]),
                );
                shared.struct_guid();
                *array = Tag::array(
                    "AuthoredEnabled",
                    "StructProperty",
                    [1_i32.to_le_bytes().to_vec(), shared.bytes()].concat(),
                );
            }
            18 => array.name = "authoredenabled",
            19 => {
                let mut value = Tag::text("VariableName", "NameProperty", &units("Ascii"));
                value.payload = [
                    (-6_i32).to_le_bytes().to_vec(),
                    units("Ascii")
                        .iter()
                        .flat_map(|u| u.to_le_bytes())
                        .collect(),
                    vec![0, 0],
                ]
                .concat();
                *array = Tag::array(
                    "AuthoredEnabled",
                    "StructProperty",
                    [
                        1_i32.to_le_bytes().to_vec(),
                        Tag::structure(
                            "EnabledElement",
                            "AuthoredBoolRecord",
                            stream(&[value, Tag::boolean("SavedBool", true)]),
                        )
                        .bytes(),
                    ]
                    .concat(),
                );
            }
            _ => {}
        }
        rejected(&mut f);
    }
}
#[test]
fn integer_array_rejects_wrong_type_width_and_header() {
    for variant in 0..5 {
        let mut f = fixture();
        let mut value = Tag::integer("SavedInt", 2);
        match variant {
            0 => value = Tag::boolean("SavedInt", true),
            1 => {
                value.payload.pop();
            }
            2 => value.payload.push(0),
            3 => value.index = 1,
            4 => value.property_guid(),
            _ => unreachable!(),
        }
        let array = f.owners[2]
            .iter_mut()
            .find(|t| t.name == "AuthoredUses")
            .unwrap();
        *array = scalar_array(
            "AuthoredUses",
            "UsesElement",
            "AuthoredIntRecord",
            value,
            &units("Usesé"),
        );
        rejected(&mut f);
    }
}
#[test]
fn whole_candidate_rejects_missing_extra_duplicate_and_unrelated_recorders() {
    for variant in 0..7 {
        let mut f = fixture();
        match variant {
            0 => f.owners[2].retain(|t| t.name != "AuthoredUses"),
            1 => {
                let tag = f.owners[2]
                    .iter()
                    .find(|t| t.name == "AuthoredEnabled")
                    .unwrap()
                    .clone();
                f.owners[2].push(tag);
            }
            2 => f.owners[2].push(Tag::array(
                "UnknownEmpty",
                "StructProperty",
                [
                    0_i32.to_le_bytes().to_vec(),
                    Tag::structure("UnknownEmpty", "Empty", vec![]).bytes(),
                ]
                .concat(),
            )),
            3 => {
                f.owners.pop();
            }
            4 => {
                let owner = f.owners[0].clone();
                f.owners.push(owner);
            }
            5 => {
                let tag = f.owners[2]
                    .iter_mut()
                    .find(|t| t.name == "ActorClassName")
                    .unwrap();
                *tag = Tag::text("ActorClassName", "NameProperty", &units("Other"));
            }
            _ => {
                let tag = f.owners[2]
                    .iter_mut()
                    .find(|t| t.name == "AuthoredUses")
                    .unwrap();
                *tag = scalar_array(
                    "AuthoredUses",
                    "UsesElement",
                    "AuthoredIntRecord",
                    Tag::integer("SavedInt", -1),
                    &units("Usesé"),
                );
            }
        }
        rejected(&mut f);
    }
}
#[test]
fn declarations_reject_collisions_limits_and_oversized_spare_capacities() {
    let f = fixture();
    for variant in 0..10 {
        let mut b = f.bindings.definition().clone();
        match variant {
            0 => b.layout.enabled.array_name = units("ActorTransform"),
            1 => b.layout.enabled.array_name = b.layout.remaining_uses.array_name.clone(),
            2 => b.layout.enabled.value_field = units("VariableName"),
            3 => b.owners[0].enabled_source_name = b.owners[0].remaining_uses_source_name.clone(),
            4 => b.owners[0].enabled_source_name = vec![65; 1025],
            5 => b.owners[0].enabled_source_name.reserve_exact(2051),
            6 => b.owners.reserve_exact(MAX_NATIVE_PLACEMENTS + 1),
            7 => b.native_contract_id.reserve_exact(65),
            8 => b.layout.remaining_uses.record_type.clear(),
            _ => b.owners[1].owner.native_placement = b.owners[0].owner.native_placement.clone(),
        }
        assert!(
            CompiledC001ScalarOwnerBindings::compile(b).is_err(),
            "variant {variant}"
        );
    }
    let mut base = f.base.clone();
    base.native_world.as_mut().unwrap().actors[0]
        .variables
        .reserve_exact(5);
    assert!(
        prepare_c001_scalar_owner_import(&f.candidate(), base, &f.bindings, &f.contracts).is_err()
    );
}
#[test]
fn provenance_is_independent_of_later_values_and_retry_preserves_them() {
    let f = fixture();
    let mut s = f.snapshot();
    s.tick = 52;
    let native = s.native_world.as_mut().unwrap();
    native.actors[0].variables = scalar_pair(true, 0);
    native.actors[0].transform.translation[0] = 3.0;
    s.validate().unwrap();
    let replay = prepare_owner_restore(&s, &f.initial_batch()).unwrap();
    assert_eq!(replay.snapshot(), &s);
    let mut corrupt = s.clone();
    corrupt
        .native_scalar_owner_import
        .as_mut()
        .unwrap()
        .source_prospect_id[0] = 65;
    assert!(corrupt.validate().is_err());
    let mut corrupt = s.clone();
    corrupt
        .native_scalar_owner_import
        .as_mut()
        .unwrap()
        .bindings
        .owners[0]
        .enabled_source_name[0] = 66;
    assert!(corrupt.validate().is_err());
    let mut corrupt = s.clone();
    corrupt.native_lifecycle.as_mut().unwrap().slots[0].component = None;
    assert!(corrupt.validate().is_err());
    let mut corrupt = s.clone();
    corrupt.native_lifecycle.as_mut().unwrap().slots[1].incarnation = 1;
    assert!(corrupt.validate().is_err());
    let mut corrupt = s.clone();
    corrupt.native_world.as_mut().unwrap().actors[0].variables[1].value = NativeValue::I32(10);
    assert!(corrupt.validate().is_err());
    let mut changed = f.initial_batch();
    changed.recorders[0].variables = scalar_pair(true, 0);
    assert!(prepare_owner_restore(&s, &changed).is_err());
}
#[test]
fn old_owner_profile_still_rejects_all_scalar_arrays() {
    let mut f = old::fixture();
    f.owners[2].push(old::Tag::array(
        "AuthoredEnabled",
        "StructProperty",
        [
            0_i32.to_le_bytes().to_vec(),
            old::Tag::structure("EnabledElement", "AuthoredBoolRecord", vec![]).bytes(),
        ]
        .concat(),
    ));
    f.rebuild_source();
    let binding =
        icarus_bevy::native_c001_owners::CompiledC001OwnerBindings::compile(f.bindings.clone())
            .unwrap();
    assert!(
        icarus_bevy::native_c001_owners::prepare_c001_owner_import(
            &f.candidate(),
            f.base.clone(),
            &binding
        )
        .is_err()
    );
}
#[test]
fn publication_preflight_rejects_existing_store_without_partial_generation() {
    let f = fixture();
    let dir = tempfile::tempdir().unwrap();
    let destination = dir.path().join("world");
    publish_new_c001_scalar_owner_import(f.prepared(), &destination).unwrap();
    let before = std::fs::read_dir(&destination)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .collect::<Vec<_>>();
    assert!(publish_new_c001_scalar_owner_import(f.prepared(), &destination).is_err());
    assert_eq!(
        std::fs::read_dir(&destination)
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect::<Vec<_>>(),
        before
    );
}
fn strict<T: serde::de::DeserializeOwned + serde::Serialize>(value: T) {
    let value = serde_json::to_value(value).unwrap();
    let mut extra = value.clone();
    extra["unknown"] = Value::Null;
    assert!(serde_json::from_value::<T>(extra).is_err());
    assert!(serde_json::from_value::<T>(json!([])).is_err());
    let key = value.as_object().unwrap().keys().next().unwrap();
    let entry = serde_json::to_string(&value[key]).unwrap();
    let text =
        serde_json::to_string(&value)
            .unwrap()
            .replacen("{", &format!("{{\"{key}\":{entry},"), 1);
    assert!(serde_json::from_str::<T>(&text).is_err());
}
#[test]
fn every_new_dto_uses_strict_object_deserialization() {
    let f = fixture();
    let s = f.snapshot();
    let m = s.native_scalar_owner_import.unwrap();
    strict::<C001ScalarArrayV1>(f.bindings.definition().layout.enabled.clone());
    strict::<C001ScalarLayoutV1>(f.bindings.definition().layout.clone());
    strict::<C001ScalarOwnerBindingV1>(f.bindings.definition().owners[0].clone());
    strict::<C001ScalarOwnerBindingsV1>(f.bindings.definition().clone());
    strict::<ResolvedC001ScalarOwnerV1>(m.resolved[0].clone());
    strict::<C001ScalarOwnerImportMetadataV1>(m);
    assert!(
        serde_json::from_value::<C001ScalarOwnerAdapter>(
            json!({"c001_authored_scalar_owners_v1":null})
        )
        .is_err()
    );
    assert!(serde_json::from_value::<C001ScalarOwnerAdapter>(json!("future")).is_err());
}
