//! Authored health-bearing fixture; expected state uses only literal arithmetic.
use super::*;
use icarus_bevy::native_player_runtime::DepletionDamageConfig;

pub(super) const PROTOCOL: &str = "authored_player_depletion_recovery_v1";

fn consumption() -> ConsumptionConfig {
    ConsumptionConfig {
        oxygen: 7,
        water: 7,
        food: 7,
    }
}
fn damage() -> DepletionDamageConfig {
    DepletionDamageConfig {
        oxygen: 2,
        water: 3,
        food: 5,
    }
}
fn checkpoint() -> PlayerCheckpoint {
    PlayerCheckpoint {
        location_cm: [0., 200.25, -300.5],
        vitals: CharacterVitals {
            health: 1_000_000,
            max_health: 1_000_000,
            stamina: 90,
            max_stamina: 100,
            oxygen: 1,
            max_oxygen: 1,
            water: 2,
            max_water: 2,
            food: 3,
            max_food: 3,
            biome_row: 0,
            growth_row: 0,
        },
    }
}
fn fixture_declaration() -> PlayerPersistenceConfig {
    PlayerPersistenceConfig {
        prospect_id: "authored-depletion-recovery".into(),
        map_url: "/Authored/DepletionRecovery".into(),
        prospect_row: "AuthoredProspect".into(),
        difficulty: 1,
    }
}
fn fixture_config() -> ServerConfig {
    let mut config = config();
    config.network.player_persistence = Some(fixture_declaration());
    config.network.player_consumption = Some(consumption());
    config.network.player_depletion_damage = Some(damage());
    config
}
fn stored_inputs(
    consumption: ConsumptionConfig,
    damage: DepletionDamageConfig,
    hz: u32,
    day: AuthoredClockConfig,
) -> Result<StoredInputs> {
    let values = [1_000_000, 1_000_000, 90, 100, 1, 1, 2, 2, 3, 3];
    Ok(StoredInputs {
        authored_gameplay: fingerprint(&(
            "authored-depletion-recovery-gameplay-v1",
            values,
            consumables::recipe(),
        ))?,
        spawn: fingerprint(&[0., 200.25, -300.5])?,
        authored_world: fingerprint(&(
            "authored-depletion-recovery-world-v1",
            day,
            authored_world(),
        ))?,
        declaration: fingerprint(&(
            "authored_player_depletion_damage_v1",
            fixture_declaration(),
            consumption,
            damage,
            hz,
        ))?,
        initial_values: fingerprint(&("AuthoredGrowth", values, 0u8))?,
    })
}
fn context_with_rules(
    consumption: ConsumptionConfig,
    damage: DepletionDamageConfig,
    hz: u32,
    day: AuthoredClockConfig,
) -> Result<PlayerContext> {
    let inputs = stored_inputs(consumption, damage, hz, day)?;
    Ok(PlayerContext::new(
        "authored-depletion-recovery".encode_utf16().collect(),
        "/Authored/DepletionRecovery".into(),
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
fn fixture_context() -> Result<PlayerContext> {
    context_with_rules(consumption(), damage(), HZ, clock())
}
pub(super) fn uses(tick: u64) -> u64 {
    tick.saturating_sub(1) / 7
}
pub(super) fn health(tick: u64) -> i32 {
    const LOSS: [u64; 8] = [0, 0, 0, 1, 2, 4, 5, 8];
    (1_000_000 - 8 * (tick / 7) - LOSS[(tick % 7) as usize]) as i32
}
fn player_revision(tick: u64) -> u64 {
    1 + tick + uses(tick)
}
pub(super) fn expected(tick: u64) -> Result<WorldSnapshot> {
    ensure!(tick <= MAX_TICK, "depletion oracle tick bound exceeded");
    let u = uses(tick);
    let r = if tick == 0 { 0 } else { 1 + (tick - 1) % 7 } as i32;
    let records = [A, B]
        .into_iter()
        .map(|id| {
            let active = id == A;
            let phase = if active { r } else { 0 };
            StoredRecord {
                account: id,
                slot: 0,
                id,
                revision: if active { player_revision(tick) } else { 1 },
                location_cm: [0., 200.25, -300.5],
                vitals: [
                    if active { health(tick) } else { 1_000_000 },
                    1_000_000,
                    90,
                    100,
                    (1 - phase).max(0),
                    1,
                    (2 - phase).max(0),
                    2,
                    (3 - phase).max(0),
                    3,
                ],
                growth_row: StoredRow {
                    table: "D_CharacterGrowth".into(),
                    row: "AuthoredGrowth".into(),
                },
            }
        })
        .collect();
    let players = StoredPlayers {
        context: StoredContext {
            prospect_id: "authored-depletion-recovery".encode_utf16().collect(),
            map_url: "/Authored/DepletionRecovery".into(),
            inputs: stored_inputs(consumption(), damage(), HZ, clock())?,
        },
        written_under: Some(StoredRelease {
            app_id: 2_089_300,
            build_id: 1,
            project_version: "authored-recovery".into(),
            network_checksum: network_checksum("authored-recovery"),
        }),
        high_water: 3,
        records,
    };
    let mut bags = Vec::new();
    for (id, token, marker, used) in [(A, 1, 2, u), (B, 3, 4, 0)] {
        bags.push(PlayerBagV1 {
            owner: owner(id),
            revision: 1 + used,
            consumed: totals(used, 0),
            inventories: vec![InventoryStateV1 {
                key: "bag".into(),
                slots: vec![
                    Some(stack(token, TOKEN, (100_000 - used) as u32)?),
                    None,
                    None,
                    None,
                ],
                overflow: vec![DeferredStackV1 {
                    stack: stack(marker, MARKER, 7)?,
                    requested_slot: 255,
                }],
            }],
        });
    }
    Ok(WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYER_INVENTORY,
        tick,
        native_players: Some(serde_json::from_slice(&serde_json::to_vec(&players)?)?),
        player_inventory: Some(PlayerInventorySnapshotV1 {
            format: 1,
            recipe_id: consumables::recipe_identity()?,
            recipe: consumables::recipe().into(),
            baseline: InventoryInitialTotalsV1 {
                items: totals(100_000, 7),
            },
            revision: 3 + u,
            next_stack_id: 5,
            bags,
        }),
        ..Default::default()
    })
}
pub(super) fn check(snapshot: &WorldSnapshot) -> Result<String> {
    ensure!(
        *snapshot == expected(snapshot.tick)?,
        "typed depletion snapshot differs at tick {}",
        snapshot.tick
    );
    check_conservation(snapshot)?;
    consumables::check_day(snapshot.tick)?;
    Ok(digest(&serde_json::to_vec(snapshot)?))
}
pub(super) fn seed() -> Result<WorldSnapshot> {
    let empty = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYER_INVENTORY,
        native_players: Some(PlayerSnapshot::fresh(fixture_context()?, &release())?),
        player_inventory: Some(
            CompiledPlayerInventoryRecipe::compile_versioned(consumables::recipe().into())?.empty(),
        ),
        ..Default::default()
    };
    let snapshot = populate_seed_with_checkpoint(empty, &fixture_config(), checkpoint())?;
    check(&snapshot)?;
    Ok(snapshot)
}
pub(super) fn open_app(snapshot: &WorldSnapshot) -> Result<App> {
    check(snapshot)?;
    let mut opened = snapshot.clone();
    opened
        .native_players
        .as_mut()
        .context("players")?
        .open(&fixture_context()?, &release())?;
    let mut app = build_app(&fixture_config(), &opened)?;
    app.finish();
    app.cleanup();
    ensure!(
        capture(app.world_mut())? == *snapshot,
        "depletion reopen changed state"
    );
    ensure!(
        !app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .reserve(key(A)?, A, checkpoint())?
            .is_new,
        "returning A missing"
    );
    for _ in 0..2 {
        activate_player(app.world_mut(), A)?;
        ensure!(
            capture(app.world_mut())? == *snapshot,
            "depletion activation changed state"
        );
    }
    Ok(app)
}
fn use_checked(app: &mut App) -> Result<()> {
    let req = request(
        app,
        A,
        PlayerInventoryActionV1::Use {
            source: endpoint(A, 1)?,
        },
    )?;
    let mut delta = capture(app.world_mut())?;
    let mut players: StoredPlayers = serde_json::from_slice(&serde_json::to_vec(
        delta.native_players.as_ref().context("players")?,
    )?)?;
    let record = &mut players.records[0];
    ensure!(record.id == A, "A identity");
    record.revision += 1;
    for (current, max) in [(4, 5), (6, 7), (8, 9)] {
        ensure!(
            record.vitals[current] < record.vitals[max],
            "Use has no deficit"
        );
        record.vitals[current] = record.vitals[max];
    }
    delta.native_players = Some(serde_json::from_slice(&serde_json::to_vec(&players)?)?);
    let inventory = delta.player_inventory.as_mut().context("inventory")?;
    inventory.revision += 1;
    let bag = &mut inventory.bags[0];
    bag.revision += 1;
    bag.consumed[1].count += 1;
    bag.inventories[0].slots[0].as_mut().context("token")?.count -= 1;
    ensure!(
        apply_player_inventory_action(app.world_mut(), key(A)?, A, &req)?
            == InventoryActionOutcomeV1::Applied,
        "Use outcome"
    );
    ensure!(capture(app.world_mut())? == delta, "Use full delta differs");
    refused(app, key(A)?, A, &req)?;
    let fresh = request(app, A, req.action)?;
    for player_stale in [false, true] {
        let mut stale = fresh.clone();
        if player_stale {
            stale.expected_player_revision -= 1;
        } else {
            stale.expected_bag_revision -= 1;
        }
        refused(app, key(A)?, A, &stale)?;
    }
    ensure!(
        apply_player_inventory_action(app.world_mut(), key(A)?, A, &fresh)?
            == InventoryActionOutcomeV1::NoChange,
        "full Use must not heal"
    );
    ensure!(capture(app.world_mut())? == delta, "full Use changed state");
    Ok(())
}
pub(super) fn step(app: &mut App) -> Result<()> {
    let previous = app.world().resource::<SimulationClock>().tick;
    ensure!(previous < MAX_TICK, "depletion tick bound exceeded");
    let tick = previous + 1;
    if tick > 1 && tick % 7 == 1 {
        use_checked(app)?;
    }
    app.world_mut().run_schedule(FixedUpdate);
    ensure!(
        app.world().resource::<SimulationClock>().tick == tick,
        "depletion fixed step differs"
    );
    check(&capture(app.world_mut())?)?;
    Ok(())
}

#[cfg(test)]
#[path = "depletion_tests.rs"]
mod tests;
