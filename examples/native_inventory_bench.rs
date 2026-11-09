//! Bounded independently authored inventory validation measurements.
use anyhow::{Result, ensure};
use clap::Parser;
use icarus_bevy::{
    native_inventory::{
        CompiledInventoryContractsV1, InitialInventoryItemV1, InitialInventoryItemsV1,
        InventoryContractsV1, InventoryDefinitionV1, InventoryLiveOwnersV1, InventoryOwnerClassV1,
        ItemDefinitionV1, SlotRuleV1, prepare_initial_inventory, validate_inventory_state,
    },
    native_lifecycle::OwnerHandle,
    native_world::CompiledNativeCatalog,
    simulation::DomainId,
};
use serde_json::json;
use std::{
    hint::black_box,
    time::{Duration, Instant},
};

#[derive(Parser)]
struct Options {
    #[arg(long, default_value_t=128, value_parser=clap::value_parser!(u16).range(1..=128))]
    owners: u16,
    #[arg(long, default_value_t=1024, value_parser=clap::value_parser!(u16).range(1..=1024))]
    items: u16,
    #[arg(long, default_value_t=20, value_parser=clap::value_parser!(u16).range(1..=100))]
    iterations: u16,
    #[arg(long, default_value_t=5, value_parser=clap::value_parser!(u16).range(3..=10))]
    samples: u16,
    #[arg(long, default_value_t=120, value_parser=clap::value_parser!(u16).range(1..=120))]
    max_seconds: u16,
}

fn main() -> Result<()> {
    let options = Options::parse();
    let started = Instant::now();
    let deadline = started + Duration::from_secs(u64::from(options.max_seconds));
    let check_deadline = || {
        ensure!(
            Instant::now() < deadline,
            "authored inventory benchmark deadline exceeded"
        );
        Ok::<_, anyhow::Error>(())
    };
    let placements: Vec<_> = (0..options.owners).map(|index| json!({
        "key":format!("owner_{index:05}"),"class":"box","tile":"tile",
        "transform":{"translation":[0.0,0.0,0.0],"rotation":[0.0,0.0,0.0,1.0],"scale":[1.0,1.0,1.0]},"initial_values":[]
    })).collect();
    let catalog = CompiledNativeCatalog::parse(&json!({
        "native_format":1,"frame":"right_handed_z_up_meters",
        "classes":[{"key":"box","kind":"spatial_object_v1","local_bounds":{"min":[-0.1,-0.1,-0.1],"max":[0.1,0.1,0.1]},"variables":[]}],
        "maps":[{"key":"map","tiles":[{"key":"tile","bounds":{"min":[-10.0,-10.0,-10.0],"max":[10.0,10.0,10.0]}}],"placements":placements}]
    }).to_string())?;
    check_deadline()?;
    let keys: Vec<_> = (0..options.items)
        .map(|index| format!("item_{index:04}"))
        .collect();
    let contracts = CompiledInventoryContractsV1::compile(
        InventoryContractsV1 {
            format: 1,
            native_catalog_id: catalog.identity().into(),
            native_map: "map".into(),
            items: keys
                .iter()
                .rev()
                .map(|key| ItemDefinitionV1 {
                    key: key.clone(),
                    stack_limit: 1,
                })
                .collect(),
            classes: vec![InventoryOwnerClassV1 {
                native_class: "box".into(),
                inventories: vec![InventoryDefinitionV1 {
                    key: "bag".into(),
                    initial_slots: 32,
                    force_add_when_zero: false,
                    overflow_limit: 0,
                    slot_rules: vec![
                        SlotRuleV1 {
                            allowed_items: keys.iter().rev().cloned().collect()
                        };
                        32
                    ],
                }],
            }],
        },
        &catalog,
    )?;
    check_deadline()?;
    let live = InventoryLiveOwnersV1 {
        owners: (1..=options.owners)
            .map(|id| OwnerHandle {
                id: DomainId(u64::from(id)),
                incarnation: 1,
            })
            .collect(),
    };
    // The final key exercises full linear scans in the historical implementation.
    let key = keys.last().unwrap();
    let initial = InitialInventoryItemsV1 {
        items: (1..=options.owners)
            .flat_map(|id| {
                (0..32).map(move |slot| InitialInventoryItemV1 {
                    owner: DomainId(u64::from(id)),
                    inventory: "bag".into(),
                    requested_slot: slot,
                    item: key.clone(),
                    count: 1,
                })
            })
            .collect(),
    };
    let prepared = prepare_initial_inventory(&contracts, &live, &initial)?;
    check_deadline()?;
    validate_inventory_state(
        &contracts,
        prepared.state(),
        prepared.initial_totals(),
        &live,
    )?;
    check_deadline()?;
    let mut sample_nanos = Vec::new();
    for _ in 0..options.samples {
        let sample = Instant::now();
        for _ in 0..options.iterations {
            check_deadline()?;
            validate_inventory_state(
                black_box(&contracts),
                black_box(prepared.state()),
                black_box(prepared.initial_totals()),
                black_box(&live),
            )?;
        }
        sample_nanos.push(sample.elapsed().as_nanos() / u128::from(options.iterations));
    }
    check_deadline()?;
    println!(
        "{}",
        json!({"event":"authored_inventory_validation_bench","benchmark_schema":1,"workload":"canonical_last_key_validation_v1","owners":options.owners,"item_types":options.items,"stacks":u32::from(options.owners)*32,"iterations":options.iterations,"sample_nanos_per_validation":sample_nanos,"contract_id":contracts.identity(),"initial_state_digest":prepared.initial_state_digest(),"compatibility_proven":false})
    );
    Ok(())
}
