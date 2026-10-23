//! Authored source-only fixture, including pure schema-10 snapshot assembly.
//! No runtime, store access or publication occurs here.
#![allow(dead_code)]
use crate::{scalar, source};
use icarus_bevy::{
    import_candidate::DecodedImportCandidate,
    native_c001_inventory_owners::{
        C001InventoryOwnerAdapter, C001InventoryOwnerBindingV1, C001InventoryOwnerBindingsV1,
        CompiledC001InventoryOwnerBindings, InventorySourceBindingV1, ItemSourceBindingV1,
        PreparedC001InventoryOwnerParts, prepare_c001_inventory_owner_parts,
    },
    native_inventory::{
        CompiledInventoryContractsV1, InventoryContractsV1, InventoryDefinitionV1,
        InventoryOwnerClassV1, ItemDefinitionV1, SlotRuleV1,
    },
    native_lifecycle::RestoreBatch,
    native_scalar_owners::CompiledScalarOwnerContracts,
    native_world::CompiledNativeCatalog,
    preservation::PreservedSection,
    simulation::{SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS, WorldSnapshot},
};
use scalar::{Tag, stream, units};
use sha2::{Digest, Sha256};
use std::sync::Arc;

pub struct Fixture {
    pub catalog: Arc<CompiledNativeCatalog>,
    pub base: WorldSnapshot,
    pub scalar_contracts: CompiledScalarOwnerContracts,
    pub inventory_contracts: CompiledInventoryContractsV1,
    pub bindings: CompiledC001InventoryOwnerBindings,
    pub payload: Vec<u8>,
    pub envelope: Vec<u8>,
    pub owners: Vec<Vec<Tag>>,
    pub roots: Vec<Tag>,
    pub info: Vec<Tag>,
    pub lifecycle_batch: RestoreBatch,
}
impl Fixture {
    pub fn candidate(&self) -> DecodedImportCandidate {
        DecodedImportCandidate::decode(self.envelope.as_slice()).unwrap()
    }
    pub fn prepared_parts(&self) -> PreparedC001InventoryOwnerParts {
        prepare_c001_inventory_owner_parts(
            &self.candidate(),
            &self.base,
            &self.bindings,
            &self.scalar_contracts,
            &self.inventory_contracts,
        )
        .unwrap()
    }
    pub fn snapshot(&self) -> WorldSnapshot {
        let candidate = self.candidate();
        let (native, lifecycle, inventories, metadata) = self.prepared_parts().into_parts();
        let source = candidate.preserved();
        let preserved = PreservedSection::from_native_encoding(
            source.native_encoding().to_vec(),
            source.payload_bytes(),
            source.payload_sha256(),
        )
        .unwrap();
        let mut snapshot = self.base.clone();
        snapshot.schema_version = SNAPSHOT_SCHEMA_C001_INVENTORY_OWNERS;
        snapshot.native_world = Some(native);
        snapshot.native_lifecycle = Some(lifecycle);
        snapshot.native_scalar_contracts = Some(self.scalar_contracts.snapshot());
        snapshot.native_inventory_contracts =
            Some(self.inventory_contracts.snapshot().as_ref().clone());
        snapshot.native_inventories = Some(inventories);
        snapshot.native_inventory_owner_import = Some(metadata);
        snapshot.preserved_prospect = Some(Arc::new(preserved));
        snapshot
    }
    pub fn rebuild_source(&mut self) {
        self.payload = payload(&self.roots, &self.info, &self.owners);
        self.envelope = scalar::envelope(&self.payload);
        let mut d = self.bindings.definition().clone();
        d.expected_payload_sha256 = Sha256::digest(&self.payload)
            .iter()
            .map(|v| format!("{v:02x}"))
            .collect();
        self.bindings = CompiledC001InventoryOwnerBindings::compile(d).unwrap();
    }
}
fn payload(roots: &[Tag], info: &[Tag], owners: &[Vec<Tag>]) -> Vec<u8> {
    let records = owners
        .iter()
        .map(|fields| {
            let bytes = [stream(fields), vec![0; 4]].concat();
            stream(&[
                Tag::text(
                    "ComponentClassName",
                    "StrProperty",
                    &units("/Script/Icarus.ActorStateRecorderComponent"),
                ),
                Tag::array(
                    "BinaryData",
                    "ByteProperty",
                    [
                        i32::try_from(bytes.len()).unwrap().to_le_bytes().to_vec(),
                        bytes,
                    ]
                    .concat(),
                ),
            ])
        })
        .collect::<Vec<_>>()
        .concat();
    let mut roots = roots.to_vec();
    roots.push(Tag::structure("ProspectInfo", "ProspectInfo", stream(info)));
    roots.push(Tag::array(
        "StateRecorderBlobs",
        "StructProperty",
        [
            i32::try_from(owners.len()).unwrap().to_le_bytes().to_vec(),
            Tag::structure("StateRecorderBlobs", "StateRecorderBlob", records).bytes(),
        ]
        .concat(),
    ));
    stream(&roots)
}
pub fn fixture() -> Fixture {
    fixture_variant(0)
}
pub fn fixture_variant(variant: u8) -> Fixture {
    let mut scalar = scalar::fixture_variant(variant);
    let lifecycle_batch = scalar.initial_batch();
    let inventory_contracts = CompiledInventoryContractsV1::compile(
        InventoryContractsV1 {
            format: 1,
            native_catalog_id: scalar.catalog.identity().into(),
            native_map: "map".into(),
            items: vec![
                ItemDefinitionV1 {
                    key: "apple".into(),
                    stack_limit: 100,
                },
                ItemDefinitionV1 {
                    key: "stone".into(),
                    stack_limit: 100,
                },
            ],
            classes: vec![InventoryOwnerClassV1 {
                native_class: "crate".into(),
                inventories: [("zero", 0, true), ("bag", 2, false)]
                    .into_iter()
                    .map(|(key, initial_slots, force)| InventoryDefinitionV1 {
                        key: key.into(),
                        initial_slots,
                        force_add_when_zero: force,
                        overflow_limit: 4,
                        slot_rules: (0..3)
                            .map(|_| SlotRuleV1 {
                                allowed_items: vec!["stone".into(), "apple".into()],
                            })
                            .collect(),
                    })
                    .collect(),
            }],
        },
        &scalar.catalog,
    )
    .unwrap();
    let row = vec![0xd804, 74];
    let second = units("AuthoredSecondRow");
    for (i, owner) in scalar.owners.iter_mut().enumerate() {
        let inventories = match i {
            // Zero-slot growth, nonzero-slot overflow, then accepted placement.
            0 => vec![
                source::inventory_fields(
                    &units("SourceZero"),
                    &[
                        source::item_fields(&row, 201, 3),
                        source::item_fields(&second, 0, 17),
                    ],
                ),
                source::inventory_fields(&units("SourceBag"), &[source::item_fields(&row, 0, 32)]),
            ],
            // Both explicit empty inventories must still be admitted and covered.
            1 => vec![
                source::inventory_fields(&units("SourceBag"), &[]),
                source::inventory_fields(&units("SourceZero"), &[]),
            ],
            _ => vec![
                source::inventory_fields(
                    &units("SourceBag"),
                    &[source::item_fields(&second, 0, 32)],
                ),
                source::inventory_fields(&units("SourceZero"), &[]),
            ],
        };
        owner.push(source::inventory_array(&inventories));
    }
    let old = scalar.bindings.definition();
    let bindings = CompiledC001InventoryOwnerBindings::compile(C001InventoryOwnerBindingsV1 {
        format: 1,
        adapter: C001InventoryOwnerAdapter::C001AuthoredInventoryOwnersV1,
        expected_payload_sha256: "0".repeat(64),
        native_catalog_id: old.native_catalog_id.clone(),
        native_lifecycle_id: old.native_lifecycle_id.clone(),
        native_scalar_contract_id: scalar.contracts.identity().into(),
        native_inventory_contract_id: inventory_contracts.identity().into(),
        source_map: old.source_map.clone(),
        native_map: old.native_map.clone(),
        coordinates: old.coordinates,
        scalar_layout: old.layout.clone(),
        inventory_layout: source::definition(),
        items: vec![
            ItemSourceBindingV1 {
                source_row: second,
                item: "stone".into(),
            },
            ItemSourceBindingV1 {
                source_row: row,
                item: "apple".into(),
            },
        ],
        owners: old
            .owners
            .iter()
            .rev()
            .map(|b| C001InventoryOwnerBindingV1 {
                scalar: b.clone(),
                inventories: vec![
                    InventorySourceBindingV1 {
                        source_key: units("SourceZero"),
                        inventory: "zero".into(),
                    },
                    InventorySourceBindingV1 {
                        source_key: units("SourceBag"),
                        inventory: "bag".into(),
                    },
                ],
            })
            .collect(),
    })
    .unwrap();
    let mut f = Fixture {
        catalog: scalar.catalog,
        base: scalar.base,
        scalar_contracts: scalar.contracts,
        inventory_contracts,
        bindings,
        payload: vec![],
        envelope: vec![],
        owners: scalar.owners,
        roots: scalar.roots,
        info: scalar.info,
        lifecycle_batch,
    };
    f.rebuild_source();
    f
}
pub fn imported() -> WorldSnapshot {
    fixture().snapshot()
}
pub fn imported_variant(variant: u8) -> WorldSnapshot {
    fixture_variant(variant).snapshot()
}
