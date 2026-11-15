//! Independent literal schema-15 fixture. No runtime replay calculates expectations.
use super::*;
use icarus_bevy::{
    native_inventory::{
        DeferredStackV1, InventoryActionOutcomeV1, InventoryDefinitionV1, InventoryInitialTotalsV1,
        InventoryStateV1, ItemDefinitionV1, ItemTotalV1, SlotRuleV1, StackId, StackV1,
    },
    native_player_inventory::{
        CompiledPlayerInventoryRecipe, PlayerBagV1, PlayerInventoryActionV1,
        PlayerInventoryOwnerV1, PlayerInventoryRecipeV1, PlayerInventoryRequestV1,
        PlayerInventorySnapshotV1, PlayerOverflowDestinationV1, PlayerSlotEndpointV1,
        PlayerStackEndpointV1, PlayerStarterItemV1,
    },
    native_player_runtime::{
        CheckpointWriter, LeavePhase, activate_player, apply_player_inventory_action,
        edit_player_checkpoint,
    },
    simulation::SNAPSHOT_SCHEMA_PLAYER_INVENTORY,
};
use std::collections::BTreeSet;

const PROTOCOL: &str = "authored_player_inventory_recovery_v1";
const A: u64 = 2;
const B: u64 = 3;
const TOKEN: &str = "authored-token";
const MARKER: &str = "authored-marker";

fn recipe() -> PlayerInventoryRecipeV1 {
    PlayerInventoryRecipeV1 {
        format: 1,
        items: vec![
            ItemDefinitionV1 {
                key: MARKER.into(),
                stack_limit: 10,
            },
            ItemDefinitionV1 {
                key: TOKEN.into(),
                stack_limit: 100_000,
            },
        ],
        inventories: vec![InventoryDefinitionV1 {
            key: "bag".into(),
            initial_slots: 4,
            slot_rules: vec![
                SlotRuleV1 {
                    allowed_items: vec![MARKER.into(), TOKEN.into()]
                };
                4
            ],
            force_add_when_zero: false,
            overflow_limit: 1,
        }],
        starter: vec![
            PlayerStarterItemV1 {
                inventory: "bag".into(),
                requested_slot: 0,
                item: TOKEN.into(),
                count: 100_000,
            },
            PlayerStarterItemV1 {
                inventory: "bag".into(),
                requested_slot: 255,
                item: MARKER.into(),
                count: 7,
            },
        ],
    }
}
fn recipe_identity() -> Result<String> {
    let mut bytes = b"authored-player-inventory-recipe-v1\0".to_vec();
    bytes.extend(serde_json::to_vec(&recipe())?);
    Ok(digest(&bytes))
}
fn fixture_context() -> Result<PlayerContext> {
    context_with_rules(rates(), HZ, clock())
}
fn context_with_rules(
    consumption: ConsumptionConfig,
    hz: u32,
    day: AuthoredClockConfig,
) -> Result<PlayerContext> {
    let inputs = stored_inputs(consumption, hz, day)?;
    Ok(PlayerContext::new(
        "authored-inventory-recovery".encode_utf16().collect(),
        "/Authored/InventoryRecovery".into(),
        PlayerInputs {
            authored_gameplay: inputs.authored_gameplay,
            spawn: inputs.spawn,
            authored_world: inputs.authored_world,
            declaration: inputs.declaration,
            initial_values: inputs.initial_values,
        },
        vec!["AuthoredGrowth".into()],
    )?)
}
fn stored_inputs(
    consumption: ConsumptionConfig,
    hz: u32,
    day: AuthoredClockConfig,
) -> Result<StoredInputs> {
    let values = [
        100, 100, 90, 100, 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000, 1_000_000,
    ];
    Ok(StoredInputs {
        authored_gameplay: fingerprint(&("authored-inventory-recovery-gameplay-v1", values))?,
        spawn: fingerprint(&[0., 200.25, -300.5])?,
        authored_world: fingerprint(&(
            "authored-inventory-recovery-world-v1",
            day,
            authored_world(),
        ))?,
        declaration: fingerprint(&(
            "authored-inventory-recovery-rules-v1",
            declaration(),
            consumption,
            hz,
            recipe(),
        ))?,
        initial_values: fingerprint(&("AuthoredGrowth", values, 0u8))?,
    })
}

// Storage-shaped local DTOs construct private PlayerSnapshot fields without
// exposing core internals or using real mutation code to calculate revisions.
#[derive(Serialize, Deserialize)]
struct StoredContext {
    prospect_id: Vec<u16>,
    map_url: String,
    inputs: StoredInputs,
}
#[derive(Serialize, Deserialize)]
struct StoredInputs {
    authored_gameplay: [u8; 32],
    spawn: [u8; 32],
    authored_world: [u8; 32],
    declaration: [u8; 32],
    initial_values: [u8; 32],
}
#[derive(Serialize, Deserialize)]
struct StoredRow {
    table: String,
    row: String,
}
#[derive(Serialize, Deserialize)]
struct StoredRelease {
    app_id: u32,
    build_id: u64,
    project_version: String,
    network_checksum: u32,
}
#[derive(Serialize, Deserialize)]
struct StoredRecord {
    account: u64,
    slot: i32,
    id: u64,
    revision: u64,
    location_cm: [f64; 3],
    vitals: [i32; 10],
    growth_row: StoredRow,
}
#[derive(Serialize, Deserialize)]
struct StoredPlayers {
    context: StoredContext,
    written_under: Option<StoredRelease>,
    high_water: u64,
    records: Vec<StoredRecord>,
}
fn survival_batches(tick: u64) -> u64 {
    // Period seven has six changed ticks; only phase one has all zero debits.
    tick / 7 * 6 + (tick % 7).saturating_sub(1)
}
fn expected_players(tick: u64, applied: u64) -> Result<PlayerSnapshot> {
    let records = [A, B]
        .into_iter()
        .map(|id| {
            let t = if id == A { tick } else { 0 };
            StoredRecord {
                account: id,
                slot: 0,
                id,
                revision: if id == A {
                    1 + tick + survival_batches(tick) + applied
                } else {
                    1
                },
                location_cm: [t as f64 * 0.25, 200.25, -300.5],
                vitals: [
                    100,
                    100,
                    90,
                    100,
                    1_000_000 - (t * 2 / 7) as i32,
                    1_000_000,
                    1_000_000 - (t * 3 / 7) as i32,
                    1_000_000,
                    1_000_000 - (t * 5 / 7) as i32,
                    1_000_000,
                ],
                growth_row: StoredRow {
                    table: "D_CharacterGrowth".into(),
                    row: "AuthoredGrowth".into(),
                },
            }
        })
        .collect();
    Ok(serde_json::from_slice(&serde_json::to_vec(
        &StoredPlayers {
            context: StoredContext {
                prospect_id: "authored-inventory-recovery".encode_utf16().collect(),
                map_url: "/Authored/InventoryRecovery".into(),
                inputs: stored_inputs(rates(), HZ, clock())?,
            },
            written_under: Some(StoredRelease {
                app_id: 2_089_300,
                build_id: 1,
                project_version: "authored-recovery".into(),
                network_checksum: network_checksum("authored-recovery"),
            }),
            high_water: 3,
            records,
        },
    )?)?)
}
fn owner(id: u64) -> PlayerInventoryOwnerV1 {
    PlayerInventoryOwnerV1 {
        player_id: id,
        incarnation: 1,
    }
}
fn stack(id: u64, item: &str, count: u32) -> Result<StackV1> {
    Ok(StackV1 {
        id: StackId::new(id)?,
        item: item.into(),
        count,
    })
}
fn totals(tokens: u64, markers: u64) -> Vec<ItemTotalV1> {
    vec![
        ItemTotalV1 {
            item: MARKER.into(),
            count: markers,
        },
        ItemTotalV1 {
            item: TOKEN.into(),
            count: tokens,
        },
    ]
}
fn expected(tick: u64) -> Result<WorldSnapshot> {
    ensure!(tick <= MAX_TICK, "inventory oracle tick bound exceeded");
    let p = tick.min(1);
    let s = (tick + 4) / 6;
    let m = (tick + 3) / 6;
    let g = (tick + 2) / 6;
    let c = (tick + 1) / 6;
    let applied = p + s + m + g + c;
    let mut a_slots = vec![
        Some(stack(
            1,
            TOKEN,
            (100_000 - c - 3 * u64::from(s > g)) as u32,
        )?),
        None,
        None,
        None,
    ];
    if s > g {
        a_slots[if s > m { 1 } else { 2 }] = Some(stack(4 + s, TOKEN, 3)?);
    }
    if p == 1 {
        a_slots[3] = Some(stack(2, MARKER, 7)?);
    }
    let overflow = |id| -> Result<Vec<DeferredStackV1>> {
        Ok(vec![DeferredStackV1 {
            stack: stack(id, MARKER, 7)?,
            requested_slot: 255,
        }])
    };
    Ok(WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYER_INVENTORY,
        tick,
        native_players: Some(expected_players(tick, applied)?),
        player_inventory: Some(PlayerInventorySnapshotV1 {
            format: 1,
            recipe_id: recipe_identity()?,
            recipe: recipe(),
            baseline: InventoryInitialTotalsV1 {
                items: totals(100_000, 7),
            },
            revision: 3 + applied,
            next_stack_id: 5 + s,
            bags: vec![
                PlayerBagV1 {
                    owner: owner(A),
                    revision: 1 + applied,
                    consumed: totals(c, 0),
                    inventories: vec![InventoryStateV1 {
                        key: "bag".into(),
                        slots: a_slots,
                        overflow: if p == 0 { overflow(2)? } else { vec![] },
                    }],
                },
                PlayerBagV1 {
                    owner: owner(B),
                    revision: 1,
                    consumed: totals(0, 0),
                    inventories: vec![InventoryStateV1 {
                        key: "bag".into(),
                        slots: vec![Some(stack(3, TOKEN, 100_000)?), None, None, None],
                        overflow: overflow(4)?,
                    }],
                },
            ],
        }),
        ..Default::default()
    })
}
fn check(snapshot: &WorldSnapshot) -> Result<String> {
    ensure!(
        *snapshot == expected(snapshot.tick)?,
        "typed inventory snapshot differs at tick {}",
        snapshot.tick
    );
    // Explicit owner-local conservation and global identity checks supplement
    // full equality and the storage decoder's shared shape validation.
    let inventory = snapshot
        .player_inventory
        .as_ref()
        .context("inventory missing")?;
    let mut ids = BTreeSet::new();
    for bag in &inventory.bags {
        for (item, baseline) in [(TOKEN, 100_000), (MARKER, 7)] {
            let live: u64 = bag
                .inventories
                .iter()
                .flat_map(|i| {
                    i.slots
                        .iter()
                        .flatten()
                        .chain(i.overflow.iter().map(|s| &s.stack))
                })
                .filter(|s| s.item == item)
                .map(|s| u64::from(s.count))
                .sum();
            let consumed = bag
                .consumed
                .iter()
                .find(|i| i.item == item)
                .context("consumed missing")?
                .count;
            ensure!(
                live + consumed == baseline,
                "owner-local conservation differs"
            );
        }
        for stack in bag.inventories.iter().flat_map(|i| {
            i.slots
                .iter()
                .flatten()
                .chain(i.overflow.iter().map(|s| &s.stack))
        }) {
            ensure!(
                ids.insert(stack.id.get()) && stack.id.get() < inventory.next_stack_id,
                "stack identity differs"
            );
        }
    }
    Ok(digest(&serde_json::to_vec(snapshot)?))
}
fn app_from(snapshot: &WorldSnapshot) -> Result<App> {
    let mut app = build_app(&config(), snapshot)?;
    app.finish();
    app.cleanup();
    Ok(app)
}
fn seed() -> Result<WorldSnapshot> {
    let empty = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYER_INVENTORY,
        native_players: Some(PlayerSnapshot::fresh(fixture_context()?, &release())?),
        player_inventory: Some(CompiledPlayerInventoryRecipe::compile(recipe())?.empty()),
        ..Default::default()
    };
    let mut app = app_from(&empty)?;
    {
        let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
        ensure!(
            registry.reserve(key(1)?, 1, initial())?.player_id == 1,
            "sparse reservation"
        );
        ensure!(registry.cancel(1), "sparse cancellation");
    }
    for id in [A, B] {
        ensure!(
            app.world_mut()
                .resource_mut::<PlayerRegistry>()
                .reserve(key(id)?, id, initial())?
                .player_id
                == id,
            "seed identity"
        );
        activate_player(app.world_mut(), id)?;
    }
    let snapshot = capture(app.world_mut())?;
    check(&snapshot)?;
    Ok(snapshot)
}
fn open_app(snapshot: &WorldSnapshot) -> Result<App> {
    check(snapshot)?;
    let mut opened = snapshot.clone();
    opened
        .native_players
        .as_mut()
        .context("players missing")?
        .open(&fixture_context()?, &release())?;
    let mut app = app_from(&opened)?;
    ensure!(
        capture(app.world_mut())? == *snapshot,
        "inventory reopen changed state"
    );
    ensure!(
        !app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .reserve(key(A)?, A, initial())?
            .is_new,
        "returning player missing"
    );
    for _ in 0..2 {
        activate_player(app.world_mut(), A)?;
        ensure!(
            capture(app.world_mut())? == *snapshot,
            "reconnect reissued inventory"
        );
    }
    Ok(app)
}
fn endpoint(id: u64, stack: u64) -> Result<PlayerStackEndpointV1> {
    Ok(PlayerStackEndpointV1 {
        owner: owner(id),
        inventory: "bag".into(),
        stack_id: StackId::new(stack)?,
    })
}
fn slot(id: u64, slot: u16) -> PlayerSlotEndpointV1 {
    PlayerSlotEndpointV1 {
        owner: owner(id),
        inventory: "bag".into(),
        slot,
    }
}
fn request(
    app: &mut App,
    id: u64,
    action: PlayerInventoryActionV1,
) -> Result<PlayerInventoryRequestV1> {
    let snapshot = capture(app.world_mut())?;
    Ok(PlayerInventoryRequestV1 {
        player_id: id,
        expected_player_revision: snapshot
            .native_players
            .as_ref()
            .context("players")?
            .projection_by_id(id)
            .context("player")?
            .revision,
        expected_bag_revision: snapshot
            .player_inventory
            .as_ref()
            .context("inventory")?
            .bag(id)
            .context("bag")?
            .revision,
        action,
    })
}
fn refused(
    app: &mut App,
    key: PlayerKey,
    epoch: u64,
    request: &PlayerInventoryRequestV1,
) -> Result<()> {
    let before = capture(app.world_mut())?;
    ensure!(
        apply_player_inventory_action(app.world_mut(), key, epoch, request).is_err(),
        "invalid request accepted"
    );
    ensure!(
        capture(app.world_mut())? == before,
        "refusal changed full state"
    );
    Ok(())
}
fn step(app: &mut App) -> Result<()> {
    let previous = app.world().resource::<SimulationClock>().tick;
    ensure!(previous < MAX_TICK, "inventory tick bound exceeded");
    let t = previous + 1;
    let projection = app
        .world()
        .resource::<PlayerRegistry>()
        .projection(A)
        .context("active A")?;
    let mut checkpoint = projection.checkpoint;
    checkpoint.location_cm[0] = t as f64 * 0.25;
    edit_player_checkpoint(app.world_mut(), key(A)?, A, projection.revision, checkpoint)?;
    let split = 4 + (t + 4) / 6;
    let action = if t == 1 {
        Some(PlayerInventoryActionV1::PlaceOverflow {
            source: endpoint(A, 2)?,
            destination: PlayerOverflowDestinationV1 {
                owner: owner(A),
                inventory: "bag".into(),
                slot: Some(3),
            },
        })
    } else {
        match t % 6 {
            2 => Some(PlayerInventoryActionV1::Split {
                source: endpoint(A, 1)?,
                destination: slot(A, 1),
                amount: 3,
            }),
            3 => Some(PlayerInventoryActionV1::Move {
                source: endpoint(A, split)?,
                destination: slot(A, 2),
            }),
            4 => Some(PlayerInventoryActionV1::Merge {
                source: endpoint(A, split)?,
                destination: endpoint(A, 1)?,
                amount: 3,
            }),
            5 => Some(PlayerInventoryActionV1::Consume {
                source: endpoint(A, 1)?,
                amount: 1,
            }),
            0 => Some(PlayerInventoryActionV1::Move {
                source: endpoint(A, 1)?,
                destination: slot(A, 0),
            }),
            _ => None,
        }
    };
    if let Some(action) = action {
        let req = request(app, A, action)?;
        let before = capture(app.world_mut())?;
        let outcome = apply_player_inventory_action(app.world_mut(), key(A)?, A, &req)?;
        ensure!(
            outcome
                == if t.is_multiple_of(6) {
                    InventoryActionOutcomeV1::NoChange
                } else {
                    InventoryActionOutcomeV1::Applied
                },
            "inventory outcome differs"
        );
        if t.is_multiple_of(6) {
            ensure!(
                capture(app.world_mut())? == before,
                "NoChange mutated state"
            );
        }
    } else {
        let valid = request(
            app,
            A,
            PlayerInventoryActionV1::Consume {
                source: endpoint(A, 1)?,
                amount: 1,
            },
        )?;
        let mut stale = valid.clone();
        stale.expected_player_revision -= 1;
        refused(app, key(A)?, A, &stale)?;
        stale = valid.clone();
        stale.expected_bag_revision -= 1;
        refused(app, key(A)?, A, &stale)?;
        let cross = request(
            app,
            A,
            PlayerInventoryActionV1::Move {
                source: endpoint(A, 1)?,
                destination: slot(B, 1),
            },
        )?;
        refused(app, key(A)?, A, &cross)?;
        refused(app, key(B)?, A, &valid)?;
        refused(app, key(A)?, 99, &valid)?;
        let detached = request(
            app,
            B,
            PlayerInventoryActionV1::Consume {
                source: endpoint(B, 3)?,
                amount: 1,
            },
        )?;
        refused(app, key(B)?, B, &detached)?;
    }
    app.world_mut().run_schedule(FixedUpdate);
    ensure!(
        app.world().resource::<SimulationClock>().tick == t,
        "inventory fixed step failed"
    );
    check(&capture(app.world_mut())?)?;
    Ok(())
}

fn load(store: &SnapshotStore, directory: &Path) -> Result<(u64, WorldSnapshot)> {
    let loaded = store.load()?.context("committed inventory missing")?;
    let manifest: Manifest = serde_json::from_slice(&fs::read(directory.join("manifest.json"))?)?;
    ensure!(manifest.schema_version == 1, "manifest schema");
    let selected = if loaded.recovered_previous {
        manifest.previous.context("previous generation missing")?
    } else {
        manifest.current
    };
    ensure!(
        selected.number > 0 && selected.sha256.len() == 64,
        "selected generation invalid"
    );
    check(&loaded.snapshot)?;
    Ok((selected.number, loaded.snapshot))
}
fn verify(directory: &Path, acknowledgements: &[Acknowledgement]) -> Result<serde_json::Value> {
    ensure!(
        !acknowledgements.is_empty(),
        "acknowledgement history missing"
    );
    let store = SnapshotStore::open_existing(directory)?;
    let (generation, snapshot) = load(&store, directory)?;
    ensure!(snapshot.tick <= MAX_TICK - 8, "verifier headroom exceeded");
    let hash = check(&snapshot)?;
    let mut previous = (0, 0);
    for ack in acknowledgements {
        ensure!(
            ack.generation == previous.0 + 1 && ack.tick >= previous.1,
            "acknowledgements out of order"
        );
        ensure!(
            ack.generation <= generation && ack.tick <= snapshot.tick,
            "acknowledged state lost"
        );
        ensure!(
            digest(&serde_json::to_vec(&expected(ack.tick)?)?) == ack.digest,
            "acknowledged digest differs"
        );
        if ack.generation == generation {
            ensure!(
                ack.tick == snapshot.tick && ack.digest == hash,
                "selected receipt differs"
            );
        }
        previous = (ack.generation, ack.tick);
    }
    ensure!(
        generation <= previous.0 + 1,
        "selection beyond single pending submission"
    );
    let mut app = open_app(&snapshot)?;
    for _ in 0..8 {
        step(&mut app)?;
    }
    let resumed = capture(app.world_mut())?;
    check(&resumed)?;
    Ok(
        json!({"generation":generation,"tick":snapshot.tick,"digest":hash,"schema":15,
        "resumed_tick":resumed.tick,"acknowledgements":acknowledgements.len()}),
    )
}
fn emit(options: &Options, event: &str, mut fields: serde_json::Value) -> Result<()> {
    super::emit_protocol(options, PROTOCOL, event, &mut fields)
}
fn matched(
    receipt: &SaveReceipt,
    pending: &Option<Acknowledgement>,
    generation: u64,
) -> Result<Acknowledgement> {
    let ack = pending
        .as_ref()
        .context("receipt without inventory submission")?;
    ensure!(
        receipt.generation == ack.generation
            && receipt.tick == ack.tick
            && receipt.generation == generation + 1,
        "inventory writer receipt mismatch"
    );
    ensure!(
        ack.digest == digest(&serde_json::to_vec(&expected(ack.tick)?)?),
        "pending inventory digest differs"
    );
    Ok(ack.clone())
}
fn acknowledge(
    options: &Options,
    receipt: SaveReceipt,
    pending: &mut Option<Acknowledgement>,
    generation: &mut u64,
) -> Result<()> {
    let ack = matched(&receipt, pending, *generation)?;
    emit(options, "save_committed", serde_json::to_value(&ack)?)?;
    *generation = receipt.generation;
    *pending = None;
    Ok(())
}
fn submit(
    app: &mut App,
    writer: &mut CheckpointWriter,
    generation: u64,
    pending: &mut Option<Acknowledgement>,
    deadline: Option<Instant>,
) -> Result<()> {
    ensure!(pending.is_none(), "pending inventory receipt overwritten");
    let snapshot = capture(app.world_mut())?;
    let ack = Acknowledgement {
        generation: generation + 1,
        tick: snapshot.tick,
        digest: check(&snapshot)?,
    };
    // No mutation between this checked capture and the writer's exclusive capture.
    match deadline {
        Some(deadline) => writer.submit_before(app.world_mut(), deadline)?,
        None => writer.submit(app.world_mut())?,
    }
    *pending = Some(ack);
    Ok(())
}
fn leave_phase(app: &App) -> Result<LeavePhase> {
    Ok(app
        .world()
        .resource::<PlayerRegistry>()
        .leave_status(key(A)?, A)
        .context("leave missing")?
        .phase)
}
fn shutdown(
    options: &Options,
    app: &mut App,
    writer: &mut CheckpointWriter,
    generation: &mut u64,
    pending: &mut Option<Acknowledgement>,
) -> Result<()> {
    let now = Instant::now();
    let deadline = now + Duration::from_secs(3);
    emit(options, "stopping", json!({}))?;
    let frozen = capture(app.world_mut())?;
    let hash = check(&frozen)?;
    let revision = frozen
        .native_players
        .as_ref()
        .context("players")?
        .projection(key(A)?)
        .context("A")?
        .revision;
    let status = app
        .world_mut()
        .resource_mut::<PlayerRegistry>()
        .request_leave(
            key(A)?,
            A,
            revision,
            now,
            deadline.saturating_duration_since(now),
        )?;
    ensure!(
        status.phase == LeavePhase::Pending && status.revision == revision,
        "leave cutoff not pending"
    );
    ensure!(capture(app.world_mut())? == frozen, "leave changed cutoff");
    if let Some(receipt) = writer.finish_before(app.world_mut(), deadline)? {
        acknowledge(options, receipt, pending, generation)?;
    }
    ensure!(
        pending.is_none() && leave_phase(app)? == LeavePhase::Pending,
        "old submission settled new leave"
    );
    submit(app, writer, *generation, pending, Some(deadline))?;
    let receipt = writer
        .finish_before(app.world_mut(), deadline)?
        .context("final inventory receipt missing")?;
    matched(&receipt, pending, *generation)?;
    ensure!(
        leave_phase(app)?
            == (LeavePhase::Durable {
                generation: receipt.generation,
                tick: receipt.tick
            }),
        "final receipt did not settle exact cutoff"
    );
    ensure!(
        receipt.tick == frozen.tick && capture(app.world_mut())? == frozen,
        "durable cutoff changed"
    );
    let status = app
        .world()
        .resource::<PlayerRegistry>()
        .leave_status(key(A)?, A)
        .context("durable leave missing")?;
    ensure!(status.revision == revision, "cutoff revision changed");
    let tick = receipt.tick;
    acknowledge(options, receipt, pending, generation)?;
    {
        let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
        ensure!(registry.detach(A), "final detach failed");
        registry.settle_leave(key(A)?, A)?;
        let counts = registry.leave_counts();
        ensure!(
            counts.pending == 0 && counts.failed == 0 && counts.durable_unsettled == 0,
            "leave obligations remain"
        );
    }
    ensure!(
        capture(app.world_mut())? == frozen,
        "settlement changed cutoff"
    );
    emit(
        options,
        "completed",
        json!({"generation":generation,"tick":tick,"digest":hash}),
    )
}
fn run(options: &Options, duration: u64, save_millis: u64, stopping: &AtomicBool) -> Result<()> {
    ensure!(
        (3..=12).contains(&duration) && (100..=5000).contains(&save_millis),
        "invalid inventory workload bounds"
    );
    let store = SnapshotStore::open_existing(&options.data_dir)?;
    let (mut generation, snapshot) = load(&store, &options.data_dir)?;
    ensure!(snapshot.tick <= MAX_TICK - 8, "run headroom exceeded");
    let mut app = open_app(&snapshot)?;
    emit(
        options,
        "started",
        json!({"generation":generation,"tick":snapshot.tick,"digest":check(&snapshot)?}),
    )?;
    let mut writer = CheckpointWriter::spawn(store)?;
    let start = Instant::now();
    let mut next_tick = start + config().tick_interval();
    let mut next_save = start + Duration::from_millis(save_millis);
    let mut pending = None;
    while !stopping.load(Ordering::SeqCst) && start.elapsed() < Duration::from_secs(duration) {
        if let Some(receipt) = writer.poll(app.world_mut())? {
            acknowledge(options, receipt, &mut pending, &mut generation)?;
        }
        if Instant::now() >= next_tick {
            ensure!(
                app.world().resource::<SimulationClock>().tick < MAX_TICK - 8,
                "inventory run exhausted verifier headroom"
            );
            step(&mut app)?;
            next_tick = Instant::now() + config().tick_interval();
        }
        if Instant::now() >= next_save && !writer.is_busy() {
            submit(&mut app, &mut writer, generation, &mut pending, None)?;
            next_save = Instant::now() + Duration::from_millis(save_millis);
        }
        thread::sleep(Duration::from_millis(2));
    }
    ensure!(
        stopping.load(Ordering::SeqCst),
        "inventory workload deadline reached before stop"
    );
    shutdown(
        options,
        &mut app,
        &mut writer,
        &mut generation,
        &mut pending,
    )
}
pub(super) fn execute(options: &Options, stopping: &AtomicBool) -> Result<()> {
    match &options.command {
        Command::Init => {
            let mut store = SnapshotStore::open(&options.data_dir)?;
            let snapshot = seed()?;
            let hash = check(&snapshot)?;
            let receipt = store.save_new(&snapshot)?;
            ensure!(
                receipt.generation == 1 && receipt.tick == 0,
                "inventory init receipt differs"
            );
            emit(
                options,
                "initialized",
                json!({"generation":receipt.generation,"tick":receipt.tick,"digest":hash}),
            )
        }
        Command::Run {
            duration_seconds,
            save_millis,
        } => run(options, *duration_seconds, *save_millis, stopping),
        Command::Verify { receipts } => {
            ensure!(
                fs::metadata(receipts)?.len() <= 1024 * 1024,
                "receipt input exceeds bound"
            );
            let acknowledgements: Vec<Acknowledgement> =
                serde_json::from_slice(&fs::read(receipts)?)?;
            emit(
                options,
                "verified",
                verify(&options.data_dir, &acknowledgements)?,
            )
        }
    }
}

#[cfg(test)]
#[path = "inventory_tests.rs"]
mod tests;
