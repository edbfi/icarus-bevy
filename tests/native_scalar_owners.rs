#[path = "common/c001_scalar_owners.rs"]
mod authored;
use authored::{fixture, scalar_pair};
use icarus_bevy::{
    native_scalar_owners::{
        CompiledScalarOwnerContracts, ScalarOwnerCallback, ScalarOwnerContractsSnapshotV1,
        ScalarOwnerContractsV1,
    },
    native_world::{CompiledNativeCatalog, NativeValue, NativeVariableValue, VariableDefinition},
};

#[test]
fn callback_evaluates_the_complete_exact_native_pair() {
    let f = fixture();
    for (enabled, count, available) in [(false, 2, false), (true, 0, false), (true, 2, true)] {
        let value = f
            .contracts
            .evaluate("crate", &scalar_pair(enabled, count))
            .unwrap();
        assert_eq!(
            (value.enabled, value.remaining_uses, value.available),
            (enabled, count, available)
        );
    }
    assert!(
        f.contracts
            .evaluate("missing", &scalar_pair(true, 1))
            .is_err()
    );
    assert!(
        f.contracts
            .evaluate("crate", &scalar_pair(true, -1))
            .is_err()
    );
    assert!(
        f.contracts
            .evaluate("crate", &scalar_pair(true, 10))
            .is_err()
    );
    assert!(
        f.contracts
            .evaluate("crate", &scalar_pair(true, 2)[..1])
            .is_err()
    );
    let mut wrong = scalar_pair(true, 2);
    wrong[1].value = NativeValue::Bool(true);
    assert!(f.contracts.evaluate("crate", &wrong).is_err());
    wrong[1] = wrong[0].clone();
    assert!(f.contracts.evaluate("crate", &wrong).is_err());
}
#[test]
fn declarations_require_exact_coverage_and_valid_defaults_and_overrides() {
    let f = fixture();
    let compile = |d| CompiledScalarOwnerContracts::compile(d, &f.catalog);
    let mut d = f.contracts.definition().clone();
    d.classes.clear();
    assert!(compile(d).is_err());
    let mut d = f.contracts.definition().clone();
    d.classes.push(d.classes[0].clone());
    assert!(compile(d).is_err());
    let mut d = f.contracts.definition().clone();
    d.classes[0].native_class = "unused".into();
    assert!(compile(d).is_err());
    let mut d = f.contracts.definition().clone();
    d.classes[0].authored_max = 0;
    assert!(compile(d).is_err());
    let mut d = f.contracts.definition().clone();
    d.classes[0].authored_max = 4;
    assert!(compile(d).is_err());
    for change in 0..4 {
        let mut catalog = f.catalog.definition().clone();
        match change {
            0 => catalog.classes[0].variables[1].default = NativeValue::I32(-1),
            1 => catalog.classes[0].variables[0].default = NativeValue::I32(1),
            2 => catalog.classes[0].variables.push(VariableDefinition {
                key: "extra".into(),
                default: NativeValue::Bool(false),
            }),
            _ => catalog.maps[0].placements[0].initial_values[0].value = NativeValue::I32(10),
        }
        let catalog = CompiledNativeCatalog::compile(catalog).unwrap();
        let mut d = f.contracts.definition().clone();
        d.native_catalog_id = catalog.identity().into();
        assert!(CompiledScalarOwnerContracts::compile(d, &catalog).is_err());
    }
}
#[test]
fn canonical_contract_identity_and_snapshot_are_verified() {
    let f = fixture();
    let text = serde_json::to_string(f.contracts.definition()).unwrap();
    let parsed = CompiledScalarOwnerContracts::parse(&text, &f.catalog).unwrap();
    assert_eq!(parsed.identity(), f.contracts.identity());
    let native = f.base.native_world.as_ref().unwrap();
    let snapshot = parsed.snapshot();
    assert_eq!(
        CompiledScalarOwnerContracts::from_snapshot(&snapshot, native)
            .unwrap()
            .identity(),
        parsed.identity()
    );
    let mut wrong = snapshot.clone();
    wrong.contract_id = "a".repeat(64);
    assert!(CompiledScalarOwnerContracts::from_snapshot(&wrong, native).is_err());
    let mut wrong = snapshot;
    wrong.definition.classes[0].authored_max = 8;
    assert!(CompiledScalarOwnerContracts::from_snapshot(&wrong, native).is_err());
}
#[test]
fn strict_objects_and_enums_reject_sequences_unknowns_and_duplicates() {
    let f = fixture();
    let value = serde_json::to_value(f.contracts.definition()).unwrap();
    let mut unknown = value.clone();
    unknown["unexpected"] = serde_json::Value::Null;
    assert!(serde_json::from_value::<ScalarOwnerContractsV1>(unknown).is_err());
    assert!(serde_json::from_str::<ScalarOwnerContractsV1>("[1,\"x\",\"map\",[]]").is_err());
    let text = serde_json::to_string(&value)
        .unwrap()
        .replacen("{", "{\"format\":1,", 1);
    assert!(serde_json::from_str::<ScalarOwnerContractsV1>(&text).is_err());
    assert!(
        serde_json::from_str::<ScalarOwnerCallback>("{\"recompute_use_availability_v1\":null}")
            .is_err()
    );
    assert!(serde_json::from_str::<ScalarOwnerCallback>("\"future_callback\"").is_err());
    let mut snapshot = serde_json::to_value(f.contracts.snapshot()).unwrap();
    snapshot["unknown"] = serde_json::json!([]);
    assert!(serde_json::from_value::<ScalarOwnerContractsSnapshotV1>(snapshot).is_err());
}
#[test]
fn oversized_spare_capacity_is_rejected() {
    let f = fixture();
    let mut definition = f.contracts.definition().clone();
    definition.classes.reserve_exact(1025);
    assert!(CompiledScalarOwnerContracts::compile(definition, &f.catalog).is_err());
    let mut definition = f.contracts.definition().clone();
    definition.native_map.reserve_exact(65);
    assert!(CompiledScalarOwnerContracts::compile(definition, &f.catalog).is_err());
    let mut native = f.base.native_world.clone().unwrap();
    native.actors[0].variables.reserve_exact(5);
    assert!(CompiledScalarOwnerContracts::from_snapshot(&f.contracts.snapshot(), &native).is_err());
    let mut variables = scalar_pair(true, 2);
    variables.push(NativeVariableValue {
        key: "third".into(),
        value: NativeValue::Bool(false),
    });
    assert!(f.contracts.evaluate("crate", &variables).is_err());
}

#[test]
fn contract_classes_are_canonicalized_before_domain_separated_digest() {
    use icarus_bevy::native_scalar_owners::ScalarOwnerClassV1;
    use sha2::{Digest, Sha256};
    let f = fixture();
    let mut definition = f.catalog.definition().clone();
    let mut second = definition.classes[0].clone();
    second.key = "second".into();
    definition.classes.push(second);
    definition.maps[0].placements[0].class = "second".into();
    let catalog = CompiledNativeCatalog::compile(definition).unwrap();
    let mut contract = f.contracts.definition().clone();
    contract.native_catalog_id = catalog.identity().into();
    contract.classes.push(ScalarOwnerClassV1 {
        native_class: "second".into(),
        authored_max: 9,
        callback: ScalarOwnerCallback::RecomputeUseAvailabilityV1,
    });
    let canonical = CompiledScalarOwnerContracts::compile(contract.clone(), &catalog).unwrap();
    contract.classes.reverse();
    let reversed = CompiledScalarOwnerContracts::compile(contract, &catalog).unwrap();
    assert_eq!(canonical.identity(), reversed.identity());
    let mut hash = Sha256::new();
    hash.update(b"icarus/native_scalar_contracts/v1\0");
    hash.update(serde_json::to_vec(canonical.definition()).unwrap());
    let expected: String = hash.finalize().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(expected, canonical.identity());
}
