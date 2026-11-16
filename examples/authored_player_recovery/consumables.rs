//! Literal format-2 consumable oracle and seven-tick authored workload.
//! Only runtime setup/actions use production mutation APIs; expectations never do.
use super::*;
use icarus_bevy::native_player_inventory::{PlayerConsumableV2, PlayerInventoryRecipeV2};

pub(super) const PROTOCOL: &str = "authored_player_consumable_recovery_v1";
const RECIPE_ID: &str = "b7e1f6e8079c3e92d3e587eef4714a65b6ffd202374ca59dfe145c42fe9abb9a";

pub(super) fn recipe() -> PlayerInventoryRecipeV2 {
    PlayerInventoryRecipeV2 {
        format: 2,
        consumables: vec![PlayerConsumableV2 {
            item: TOKEN.into(),
            oxygen: 11,
            water: 13,
            food: 17,
        }],
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
pub(super) fn recipe_identity() -> Result<String> {
    let mut bytes = b"authored-player-inventory-recipe-v2\0".to_vec();
    bytes.extend(serde_json::to_vec(&recipe())?);
    ensure!(
        digest(&bytes) == RECIPE_ID,
        "literal consumable recipe digest differs"
    );
    Ok(RECIPE_ID.into())
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
        "authored-consumable-recovery".encode_utf16().collect(),
        "/Authored/ConsumableRecovery".into(),
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
        authored_gameplay: fingerprint(&("authored-consumable-recovery-gameplay-v1", values))?,
        spawn: fingerprint(&[0., 200.25, -300.5])?,
        authored_world: fingerprint(&(
            "authored-consumable-recovery-world-v1",
            day,
            authored_world(),
        ))?,
        declaration: fingerprint(&(
            "authored-consumable-recovery-rules-v1",
            fixture_declaration(),
            consumption,
            hz,
            recipe(),
        ))?,
        initial_values: fingerprint(&("AuthoredGrowth", values, 0u8))?,
    })
}

fn expected_players(tick: u64, applied: u64) -> Result<PlayerSnapshot> {
    let u = uses(tick);
    // Use resets before fixed tick L=7u-2, so debit restarts at q=L-1.
    // Deficits before first Use are [1,1,2]; later maxima [2,3,5] are
    // below [11,13,17], making every scheduled Use saturate all currents.
    let q = if u == 0 { 0 } else { 7 * u - 3 };
    let records = [A, B]
        .into_iter()
        .map(|id| {
            let t = if id == A { tick } else { 0 };
            let anchor = if id == A { q } else { 0 };
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
                    1_000_000 - (t * 2 / 7 - anchor * 2 / 7) as i32,
                    1_000_000,
                    1_000_000 - (t * 3 / 7 - anchor * 3 / 7) as i32,
                    1_000_000,
                    1_000_000 - (t * 5 / 7 - anchor * 5 / 7) as i32,
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
                prospect_id: "authored-consumable-recovery".encode_utf16().collect(),
                map_url: "/Authored/ConsumableRecovery".into(),
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
pub(super) fn expected(tick: u64) -> Result<WorldSnapshot> {
    ensure!(tick <= MAX_TICK, "inventory oracle tick bound exceeded");
    let p = tick.min(1);
    let s = (tick + 5) / 7;
    let m = (tick + 4) / 7;
    let g = (tick + 3) / 7;
    let u = uses(tick);
    let c = (tick + 1) / 7;
    let applied = p + s + m + g + u + c;
    let mut a_slots = vec![
        Some(stack(
            1,
            TOKEN,
            (100_000 - u - c - 3 * u64::from(s > g)) as u32,
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
        player_inventory: Some(
            PlayerInventorySnapshotV1 {
                format: 1,
                recipe_id: recipe_identity()?,
                recipe: recipe().into(),
                baseline: InventoryInitialTotalsV1 {
                    items: totals(100_000, 7),
                },
                revision: 3 + applied,
                next_stack_id: 5 + s,
                bags: vec![
                    PlayerBagV1 {
                        owner: owner(A),
                        revision: 1 + applied,
                        consumed: totals(u + c, 0),
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
            }
            .into(),
        ),
        ..Default::default()
    })
}

fn fixture_declaration() -> PlayerPersistenceConfig {
    PlayerPersistenceConfig {
        prospect_id: "authored-consumable-recovery".into(),
        map_url: "/Authored/ConsumableRecovery".into(),
        prospect_row: "AuthoredProspect".into(),
        difficulty: 1,
    }
}
fn fixture_config() -> ServerConfig {
    let mut config = config();
    config.network.player_persistence = Some(fixture_declaration());
    config
}
pub(super) fn uses(tick: u64) -> u64 {
    (tick + 2) / 7
}
fn applied(t: u64) -> u64 {
    t.min(1) + (t + 5) / 7 + (t + 4) / 7 + (t + 3) / 7 + uses(t) + (t + 1) / 7
}
fn player_revision(t: u64) -> u64 {
    1 + t + survival_batches(t) + applied(t)
}
pub(super) fn check_day(tick: u64) -> Result<u32> {
    ensure!(tick <= MAX_TICK, "day tick bound exceeded");
    // Independent rational arithmetic; convert exactly once, including +0.
    let minute = ((36_960 + 1440 * (tick % 77)) % 110_880) as f64 / 77.0;
    let expected = if minute == 0.0 { 0.0f32 } else { minute as f32 };
    let actual = clock().prepare()?.sample(tick).to_bits();
    ensure!(
        actual == expected.to_bits(),
        "authored day differs at {tick}"
    );
    Ok(actual)
}
pub(super) fn check(snapshot: &WorldSnapshot) -> Result<String> {
    ensure!(
        *snapshot == expected(snapshot.tick)?,
        "typed consumable snapshot differs at tick {}",
        snapshot.tick
    );
    check_conservation(snapshot)?;
    check_day(snapshot.tick)?;
    Ok(digest(&serde_json::to_vec(snapshot)?))
}
pub(super) fn seed() -> Result<WorldSnapshot> {
    let empty = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYER_INVENTORY,
        native_players: Some(PlayerSnapshot::fresh(fixture_context()?, &release())?),
        player_inventory: Some(
            CompiledPlayerInventoryRecipe::compile_versioned(recipe().into())?
                .empty()
                .into(),
        ),
        ..Default::default()
    };
    let snapshot = populate_seed(empty, &fixture_config())?;
    check(&snapshot)?;
    Ok(snapshot)
}
pub(super) fn open_app(snapshot: &WorldSnapshot) -> Result<App> {
    check(snapshot)?;
    let mut app = {
        let mut opened = snapshot.clone();
        opened
            .native_players
            .as_mut()
            .context("players")?
            .open(&fixture_context()?, &release())?;
        let mut app = build_app(&fixture_config(), &opened)?;
        app.finish();
        app.cleanup();
        app
    };
    ensure!(
        capture(app.world_mut())? == *snapshot,
        "reopen changed state"
    );
    check_day(snapshot.tick)?;
    ensure!(
        !app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .reserve(key(A)?, A, initial())?
            .is_new,
        "returning A missing"
    );
    for _ in 0..2 {
        activate_player(app.world_mut(), A)?;
        ensure!(
            capture(app.world_mut())? == *snapshot,
            "activation changed state"
        );
    }
    if uses(snapshot.tick) > 0 {
        let last = 7 * uses(snapshot.tick) - 2;
        let old = PlayerInventoryRequestV1 {
            player_id: A,
            expected_player_revision: player_revision(last) - 2,
            expected_bag_revision: applied(last),
            action: PlayerInventoryActionV1::Use {
                source: endpoint(A, 1)?,
            },
        };
        stale_checks(&mut app, &old)?;
    }
    Ok(app)
}
fn stale_checks(app: &mut App, old: &PlayerInventoryRequestV1) -> Result<()> {
    refused(app, key(A)?, A, old)?;
    let fresh = request(app, A, old.action.clone())?;
    let mut stale = old.clone();
    stale.expected_player_revision = fresh.expected_player_revision;
    refused(app, key(A)?, A, &stale)?;
    stale = old.clone();
    stale.expected_bag_revision = fresh.expected_bag_revision;
    refused(app, key(A)?, A, &stale)
}
fn invalid_use_checks(app: &mut App) -> Result<()> {
    let valid = request(
        app,
        A,
        PlayerInventoryActionV1::Use {
            source: endpoint(A, 1)?,
        },
    )?;
    let mut stale = valid.clone();
    stale.expected_player_revision -= 1;
    refused(app, key(A)?, A, &stale)?;
    stale = valid.clone();
    stale.expected_bag_revision -= 1;
    refused(app, key(A)?, A, &stale)?;
    refused(app, key(1)?, A, &valid)?;
    refused(app, key(B)?, A, &valid)?;
    refused(app, key(A)?, 99, &valid)?;
    for mut source in [
        endpoint(A, 999)?,
        endpoint(A, 1)?,
        endpoint(B, 3)?,
        endpoint(A, 2)?,
    ] {
        // Distinguish missing inventory and wrong incarnation from missing stack.
        if source.stack_id.get() == 1 {
            source.inventory = "missing".into();
        }
        let bad = request(app, A, PlayerInventoryActionV1::Use { source })?;
        refused(app, key(A)?, A, &bad)?;
    }
    let mut source = endpoint(A, 1)?;
    source.owner.incarnation = 2;
    let bad = request(app, A, PlayerInventoryActionV1::Use { source })?;
    refused(app, key(A)?, A, &bad)?;
    let detached = request(
        app,
        B,
        PlayerInventoryActionV1::Use {
            source: endpoint(B, 3)?,
        },
    )?;
    refused(app, key(B)?, B, &detached)
}
/// Assert the exact coupled pre-Finish action, preserving every unrelated field.
fn consume_checked(app: &mut App, use_effect: bool) -> Result<()> {
    let action = if use_effect {
        PlayerInventoryActionV1::Use {
            source: endpoint(A, 1)?,
        }
    } else {
        PlayerInventoryActionV1::Consume {
            source: endpoint(A, 1)?,
            amount: 1,
        }
    };
    let req = request(app, A, action)?;
    {
        let mut expected_delta = capture(app.world_mut())?;
        let mut players: StoredPlayers = serde_json::from_slice(&serde_json::to_vec(
            expected_delta.native_players.as_ref().context("players")?,
        )?)?;
        let record = &mut players.records[0];
        ensure!(record.id == A, "sparse A identity");
        record.revision += 1;
        if use_effect {
            for (current, max) in [(4, 5), (6, 7), (8, 9)] {
                ensure!(
                    record.vitals[current] < record.vitals[max],
                    "Use had no deficit"
                );
                record.vitals[current] = record.vitals[max];
            }
        }
        expected_delta.native_players =
            Some(serde_json::from_slice(&serde_json::to_vec(&players)?)?);
        drop(players);
        let inventory = expected_delta
            .player_inventory
            .as_mut()
            .context("inventory")?
            .as_v1_mut()
            .context("inventory format")?;
        inventory.revision += 1;
        let bag = &mut inventory.bags[0];
        bag.revision += 1;
        bag.consumed[1].count += 1;
        bag.inventories[0].slots[0].as_mut().context("token")?.count -= 1;
        ensure!(
            apply_player_inventory_action(app.world_mut(), key(A)?, A, &req)?
                == InventoryActionOutcomeV1::Applied,
            "consume outcome"
        );
        ensure!(
            capture(app.world_mut())? == expected_delta,
            "coupled action delta differs"
        );
    }
    if use_effect {
        stale_checks(app, &req)?;
        let fresh = request(app, A, req.action)?;
        for _ in 0..2 {
            let before = capture(app.world_mut())?;
            ensure!(
                apply_player_inventory_action(app.world_mut(), key(A)?, A, &fresh)?
                    == InventoryActionOutcomeV1::NoChange,
                "full Use outcome"
            );
            ensure!(
                capture(app.world_mut())? == before,
                "full Use changed state"
            );
        }
        invalid_use_checks(app)?;
    }
    Ok(())
}
pub(super) fn step(app: &mut App) -> Result<()> {
    let previous = app.world().resource::<SimulationClock>().tick;
    ensure!(previous < MAX_TICK, "consumable tick bound exceeded");
    let t = previous + 1;
    let projection = app
        .world()
        .resource::<PlayerRegistry>()
        .projection(A)
        .context("active A")?;
    let mut checkpoint = projection.checkpoint;
    checkpoint.location_cm[0] = t as f64 * 0.25;
    edit_player_checkpoint(app.world_mut(), key(A)?, A, projection.revision, checkpoint)?;
    let split = 4 + (t + 5) / 7;
    if t % 7 == 5 || t % 7 == 6 {
        consume_checked(app, t % 7 == 5)?;
    } else {
        let action = if t == 1 {
            PlayerInventoryActionV1::PlaceOverflow {
                source: endpoint(A, 2)?,
                destination: PlayerOverflowDestinationV1 {
                    owner: owner(A),
                    inventory: "bag".into(),
                    slot: Some(3),
                },
            }
        } else {
            match t % 7 {
                2 => PlayerInventoryActionV1::Split {
                    source: endpoint(A, 1)?,
                    destination: slot(A, 1),
                    amount: 3,
                },
                3 => PlayerInventoryActionV1::Move {
                    source: endpoint(A, split)?,
                    destination: slot(A, 2),
                },
                4 => PlayerInventoryActionV1::Merge {
                    source: endpoint(A, split)?,
                    destination: endpoint(A, 1)?,
                    amount: 3,
                },
                _ => PlayerInventoryActionV1::Move {
                    source: endpoint(A, 1)?,
                    destination: slot(A, 0),
                },
            }
        };
        let req = request(app, A, action)?;
        let no_change = t != 1 && (t.is_multiple_of(7) || t % 7 == 1);
        let before = if no_change {
            Some(capture(app.world_mut())?)
        } else {
            None
        };
        ensure!(
            apply_player_inventory_action(app.world_mut(), key(A)?, A, &req)?
                == if no_change {
                    InventoryActionOutcomeV1::NoChange
                } else {
                    InventoryActionOutcomeV1::Applied
                },
            "phase outcome differs"
        );
        if let Some(before) = before {
            ensure!(
                capture(app.world_mut())? == before,
                "same-slot move changed state"
            );
            drop(before);
            invalid_use_checks(app)?;
        }
    }
    app.world_mut().run_schedule(FixedUpdate);
    ensure!(
        app.world().resource::<SimulationClock>().tick == t,
        "fixed step differs"
    );
    check(&capture(app.world_mut())?)?;
    Ok(())
}

#[cfg(test)]
#[path = "consumables_tests.rs"]
mod tests;
