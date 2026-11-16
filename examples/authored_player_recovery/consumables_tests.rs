use super::*;

fn mutate_players(
    snapshot: &mut WorldSnapshot,
    mutation: impl FnOnce(&mut StoredPlayers),
) -> Result<()> {
    let mut stored: StoredPlayers = serde_json::from_slice(&serde_json::to_vec(
        snapshot.native_players.as_ref().context("players")?,
    )?)?;
    mutation(&mut stored);
    snapshot.native_players = Some(serde_json::from_slice(&serde_json::to_vec(&stored)?)?);
    Ok(())
}

#[test]
fn full_typed_oracle_rejects_independent_field_mutations() -> Result<()> {
    let good = expected(9)?;
    good.validate()?;
    macro_rules! reject {
        ($snapshot:ident, $body:block) => {{
            let mut $snapshot = good.clone();
            $body
            assert!(check(&$snapshot).is_err(), "{}", stringify!($body));
        }};
    }
    reject!(bad, {
        bad.schema_version = 14;
    });
    reject!(bad, {
        bad.tick += 1;
    });
    reject!(bad, {
        bad.native_players = None;
    });
    reject!(bad, {
        mutate_players(&mut bad, |p| p.high_water += 1)?;
        bad.validate()?;
    });
    reject!(bad, {
        mutate_players(&mut bad, |p| p.records[0].id = 1)?;
    });
    reject!(bad, {
        mutate_players(&mut bad, |p| p.records[0].revision += 1)?;
        bad.validate()?;
    });
    reject!(bad, {
        mutate_players(&mut bad, |p| p.records[0].location_cm[0] += 0.25)?;
    });
    reject!(bad, {
        mutate_players(&mut bad, |p| p.records[0].vitals[8] -= 1)?;
    });
    reject!(bad, {
        mutate_players(&mut bad, |p| p.records[1].vitals[4] -= 1)?;
    });
    reject!(bad, {
        mutate_players(&mut bad, |p| {
            p.written_under.as_mut().unwrap().build_id += 1
        })?;
    });
    reject!(bad, {
        mutate_players(&mut bad, |p| p.context.inputs.spawn[0] ^= 1)?;
        bad.validate()?;
    });
    reject!(bad, {
        mutate_players(&mut bad, |p| p.context.inputs.initial_values[0] ^= 1)?;
        bad.validate()?;
    });
    reject!(bad, {
        mutate_players(&mut bad, |p| p.context.prospect_id.push(65))?;
    });
    macro_rules! inventory_reject {
        ($inv:ident, $body:block) => {
            reject!(bad, {
                let $inv = bad.player_inventory.as_mut().unwrap().as_v1_mut().unwrap();
                $body
            });
        };
    }
    inventory_reject!(i, {
        i.bags[0].owner.incarnation += 1;
    });
    inventory_reject!(i, {
        i.bags[0].owner.player_id = B;
    });
    inventory_reject!(i, {
        i.bags[0].revision += 1;
    });
    inventory_reject!(i, {
        i.revision += 1;
    });
    inventory_reject!(i, {
        i.next_stack_id -= 1;
    });
    reject!(bad, {
        bad.player_inventory
            .as_mut()
            .unwrap()
            .as_v1_mut()
            .unwrap()
            .next_stack_id += 1;
        bad.validate()?;
    });
    inventory_reject!(i, {
        i.recipe_id = "0".repeat(64);
    });
    inventory_reject!(i, {
        let icarus_bevy::native_player_inventory::PlayerInventoryRecipe::V2(recipe) = &mut i.recipe
        else {
            unreachable!()
        };
        recipe.consumables[0].food += 1;
    });
    inventory_reject!(i, {
        i.baseline.items[0].count += 1;
    });
    inventory_reject!(i, {
        i.bags[0].inventories[0].slots.swap(1, 2);
    });
    inventory_reject!(i, {
        i.bags[1].inventories[0].overflow[0].requested_slot = 254;
    });
    inventory_reject!(i, {
        i.bags[1].inventories[0].overflow.clear();
    });
    inventory_reject!(i, {
        i.bags[0].inventories[0].slots[0].as_mut().unwrap().id = StackId::new(99)?;
    });
    inventory_reject!(i, {
        i.bags[0].inventories[0].slots[0].as_mut().unwrap().count -= 1;
    });
    inventory_reject!(i, {
        i.bags[0].consumed[1].count += 1;
    });
    inventory_reject!(i, {
        i.bags[1].revision += 1;
    });
    // A global quantity-preserving transfer still violates each owner's baseline.
    reject!(bad, {
        let i = bad.player_inventory.as_mut().unwrap().as_v1_mut().unwrap();
        i.bags[0].inventories[0].slots[0].as_mut().unwrap().count += 1;
        i.bags[1].inventories[0].slots[0].as_mut().unwrap().count -= 1;
    });
    reject!(bad, {
        bad.entities.push(icarus_bevy::simulation::EntityRecord {
            id: icarus_bevy::simulation::DomainId(1),
            position: icarus_bevy::simulation::Position([0.; 3]),
        });
        bad.validate()?;
    });
    Ok(())
}

fn options(directory: &Path) -> Options {
    Options {
        data_dir: directory.into(),
        run_id: "consumable-unit".into(),
        command: Command::Init,
    }
}

#[test]
fn exact_new_submission_is_required_for_same_tick_leave() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = open_app(&seed()?)?;
    for _ in 0..5 {
        step(&mut app)?;
    }
    let frozen = capture(app.world_mut())?;
    let mut writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path())?)?;
    let mut pending = None;
    submit(
        InventoryMode::Consumables,
        &mut app,
        &mut writer,
        0,
        &mut pending,
        None,
    )?;
    let revision = frozen
        .native_players
        .as_ref()
        .unwrap()
        .projection(key(A)?)
        .unwrap()
        .revision;
    let now = Instant::now();
    let deadline = now + Duration::from_secs(3);
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .request_leave(key(A)?, A, revision, now, Duration::from_secs(3))?;
    let req = request(
        &mut app,
        A,
        PlayerInventoryActionV1::Use {
            source: endpoint(A, 1)?,
        },
    )?;
    refused(&mut app, key(A)?, A, &req)?;
    assert!(activate_player(app.world_mut(), A).is_err());
    assert!(edit_player_checkpoint(app.world_mut(), key(A)?, A, revision, initial()).is_err());
    assert_eq!(
        icarus_bevy::native_player_runtime::consume_player_vitals(
            app.world_mut(),
            icarus_bevy::native_player_runtime::ConsumptionRates::new(2, 3, 5)?
        )?,
        0
    );
    assert_eq!(capture(app.world_mut())?, frozen);
    let old = writer
        .finish_before(app.world_mut(), deadline)?
        .context("old receipt")?;
    matched(InventoryMode::Consumables, &old, &pending, 0)?;
    assert_eq!(leave_phase(&app)?, LeavePhase::Pending);
    let generation = old.generation;
    pending = None;
    submit(
        InventoryMode::Consumables,
        &mut app,
        &mut writer,
        generation,
        &mut pending,
        Some(deadline),
    )?;
    let final_receipt = writer
        .finish_before(app.world_mut(), deadline)?
        .context("final receipt")?;
    matched(
        InventoryMode::Consumables,
        &final_receipt,
        &pending,
        generation,
    )?;
    assert_eq!(
        leave_phase(&app)?,
        LeavePhase::Durable {
            generation: final_receipt.generation,
            tick: frozen.tick
        }
    );
    assert_eq!(capture(app.world_mut())?, frozen);
    let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
    assert!(registry.detach(A));
    registry.settle_leave(key(A)?, A)?;
    let counts = registry.leave_counts();
    assert_eq!(
        (counts.pending, counts.failed, counts.durable_unsettled),
        (0, 0, 0)
    );
    drop(registry);
    assert_eq!(capture(app.world_mut())?, frozen);
    Ok(())
}

#[test]
fn mismatched_receipt_and_deadline_cannot_acknowledge_or_settle() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let options = options(directory.path());
    let ack = Acknowledgement {
        generation: 1,
        tick: 0,
        digest: check(&expected(0)?)?,
    };
    let mut pending = Some(ack.clone());
    let mut generation = 0;
    assert!(
        acknowledge(
            InventoryMode::Consumables,
            &options,
            SaveReceipt {
                generation: 1,
                tick: 1
            },
            &mut pending,
            &mut generation
        )
        .is_err()
    );
    assert_eq!(generation, 0);
    assert!(pending.is_some());
    assert!(
        acknowledge(
            InventoryMode::Consumables,
            &options,
            SaveReceipt {
                generation: 2,
                tick: 0
            },
            &mut pending,
            &mut generation
        )
        .is_err()
    );
    assert_eq!(generation, 0);
    assert!(pending.is_some());
    pending.as_mut().unwrap().digest = "0".repeat(64);
    assert!(
        acknowledge(
            InventoryMode::Consumables,
            &options,
            SaveReceipt {
                generation: 1,
                tick: 0
            },
            &mut pending,
            &mut generation
        )
        .is_err()
    );
    assert_eq!(generation, 0);
    assert!(pending.is_some());
    assert!(
        matched(
            InventoryMode::Consumables,
            &SaveReceipt {
                generation: 1,
                tick: 0
            },
            &None,
            0
        )
        .is_err()
    );
    let mut app = open_app(&seed()?)?;
    let now = Instant::now();
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .request_leave(key(A)?, A, 1, now, Duration::from_secs(3))?;
    let mut writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path())?)?;
    pending = None;
    assert!(
        submit(
            InventoryMode::Consumables,
            &mut app,
            &mut writer,
            0,
            &mut pending,
            Some(now)
        )
        .is_err()
    );
    assert!(pending.is_none());
    assert!(!matches!(leave_phase(&app)?, LeavePhase::Durable { .. }));
    assert!(
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .settle_leave(key(A)?, A)
            .is_err()
    );
    assert_eq!(capture(app.world_mut())?, expected(0)?);
    Ok(())
}

#[test]
fn shutdown_settles_exact_cutoff_and_aborts_after_bad_old_receipt() -> Result<()> {
    for corrupt in [false, true] {
        let directory = tempfile::tempdir()?;
        let mut app = open_app(&seed()?)?;
        for _ in 0..12 {
            step(&mut app)?;
        }
        let frozen = capture(app.world_mut())?;
        let mut writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path())?)?;
        let mut generation = 0;
        let mut pending = None;
        submit(
            InventoryMode::Consumables,
            &mut app,
            &mut writer,
            generation,
            &mut pending,
            None,
        )?;
        if corrupt {
            pending.as_mut().unwrap().tick += 1;
        }
        let result = shutdown(
            InventoryMode::Consumables,
            &options(directory.path()),
            &mut app,
            &mut writer,
            &mut generation,
            &mut pending,
        );
        assert_eq!(capture(app.world_mut())?, frozen);
        let counts = app.world().resource::<PlayerRegistry>().leave_counts();
        if corrupt {
            assert!(result.is_err());
            assert_eq!(generation, 0);
            assert!(pending.is_some());
            assert_eq!(leave_phase(&app)?, LeavePhase::Pending);
            assert_eq!((counts.pending, counts.durable), (1, 0));
        } else {
            result?;
            assert_eq!(generation, 2);
            assert!(pending.is_none());
            assert_eq!(
                (
                    counts.pending,
                    counts.failed,
                    counts.durable_unsettled,
                    counts.detached_settled
                ),
                (0, 0, 0, 1)
            );
        }
    }
    Ok(())
}

#[test]
fn literal_seed_witnesses_and_two_day_periods() -> Result<()> {
    assert_eq!(
        recipe_identity()?,
        "b7e1f6e8079c3e92d3e587eef4714a65b6ffd202374ca59dfe145c42fe9abb9a"
    );
    assert_eq!(
        CompiledPlayerInventoryRecipe::compile_versioned(recipe().into())?.identity(),
        recipe_identity()?
    );
    assert_eq!(seed()?, expected(0)?);
    for (t, a, b, rev, next, deficits) in [
        (0, 0, 0, 1, 5, [0, 0, 0]),
        (4, 4, 3, 12, 6, [1, 1, 2]),
        (5, 5, 4, 15, 6, [0, 1, 1]),
        (6, 6, 5, 18, 6, [0, 1, 2]),
        (7, 6, 6, 20, 6, [1, 2, 3]),
        (11, 9, 9, 30, 7, [2, 3, 5]),
        (12, 10, 10, 33, 7, [0, 1, 1]),
    ] {
        let snapshot = expected(t)?;
        let players = snapshot.native_players.as_ref().unwrap();
        let p = players.projection_by_id(A).unwrap();
        assert_eq!(p.revision, rev);
        let v = p.checkpoint.vitals;
        assert_eq!(
            [
                v.max_oxygen - v.oxygen,
                v.max_water - v.water,
                v.max_food - v.food
            ],
            deficits
        );
        assert_eq!(applied(t), a);
        assert_eq!(survival_batches(t), b);
        let inv = snapshot.player_inventory.as_ref().unwrap().as_v1().unwrap();
        assert_eq!(
            (inv.revision, inv.bags[0].revision, inv.next_stack_id),
            (3 + a, 1 + a, next)
        );
    }
    let mut app = open_app(&seed()?)?;
    for t in 0..=154 {
        assert_eq!(capture(app.world_mut())?, expected(t)?);
        assert_eq!(check_day(t)?, check_day(t % 77)?);
        if t < 154 {
            step(&mut app)?;
        }
    }
    expected(MAX_TICK)?.validate()?;
    check_day(MAX_TICK)?;
    assert!(expected(MAX_TICK + 1).is_err());
    let mut app = open_app(&expected(MAX_TICK)?)?;
    assert!(step(&mut app).is_err());
    assert_eq!(capture(app.world_mut())?, expected(MAX_TICK)?);
    Ok(())
}

#[test]
fn every_reopened_phase_and_day_wrap_resumes_eight_ticks_without_writes() -> Result<()> {
    for tick in (0..=14).chain([76, 77, 78]) {
        let directory = tempfile::tempdir()?;
        let snapshot = expected(tick)?;
        let ack = {
            let receipt = SnapshotStore::open(directory.path())?.save_new(&snapshot)?;
            Acknowledgement {
                generation: receipt.generation,
                tick,
                digest: check(&snapshot)?,
            }
        };
        let manifest = fs::read(directory.path().join("manifest.json"))?;
        let bytes = fs::read(directory.path().join("snapshot-1.json"))?;
        let result = verify(InventoryMode::Consumables, directory.path(), &[ack])?;
        assert_eq!(result["resumed_tick"], tick + 8);
        assert_eq!(result["uses"], uses(tick));
        assert_eq!(result["resumed_uses"], uses(tick + 8));
        assert_eq!(result["day_sample_bits"], check_day(tick)?);
        assert_eq!(result["resumed_day_sample_bits"], check_day(tick + 8)?);
        assert_eq!(fs::read(directory.path().join("manifest.json"))?, manifest);
        assert_eq!(fs::read(directory.path().join("snapshot-1.json"))?, bytes);
        let mut reopened = open_app(&snapshot)?;
        assert_eq!(capture(reopened.world_mut())?, snapshot);
        assert_eq!(
            reopened
                .world()
                .resource::<PlayerRegistry>()
                .activated_epochs(),
            vec![A]
        );
        for t in tick + 1..=tick + 8 {
            step(&mut reopened)?;
            assert_eq!(capture(reopened.world_mut())?, expected(t)?);
        }
    }
    Ok(())
}

#[test]
fn use_saturates_before_debit_raw_consume_has_no_gain_and_retries_preserve_state() -> Result<()> {
    for tick in [4, 11, 18] {
        let mut app = open_app(&expected(tick)?)?;
        // Actual action probes occur before Finish, so all three are full here.
        consume_checked(&mut app, true)?;
        let v = app
            .world()
            .resource::<PlayerRegistry>()
            .projection(A)
            .unwrap()
            .checkpoint
            .vitals;
        assert_eq!([v.oxygen, v.water, v.food], [1_000_000; 3]);
        invalid_use_checks(&mut app)?;
        // Location is the other pre-Finish operation in the normal workload.
        let p = app
            .world()
            .resource::<PlayerRegistry>()
            .projection(A)
            .unwrap();
        let mut cp = p.checkpoint;
        cp.location_cm[0] = (tick + 1) as f64 * 0.25;
        edit_player_checkpoint(app.world_mut(), key(A)?, A, p.revision, cp)?;
        app.world_mut().run_schedule(FixedUpdate);
        assert_eq!(capture(app.world_mut())?, expected(tick + 1)?);
        step(&mut app)?; // Raw Consume: consume_checked compares every pre-Finish field.
        assert_eq!(capture(app.world_mut())?, expected(tick + 2)?);
    }
    Ok(())
}

#[test]
fn context_binds_clock_rates_hz_effect_and_modes_without_mutating_source() -> Result<()> {
    let snapshot = expected(12)?;
    let players = snapshot.native_players.as_ref().unwrap();
    for different in [
        context_with_rules(
            ConsumptionConfig {
                oxygen: 3,
                ..rates()
            },
            HZ,
            clock(),
        )?,
        context_with_rules(rates(), HZ + 1, clock())?,
        context_with_rules(
            rates(),
            HZ,
            AuthoredClockConfig {
                seconds_per_day: 12,
                ..clock()
            },
        )?,
        super::super::fixture_context()?,
    ] {
        let mut candidate = players.clone();
        assert!(candidate.open(&different, &release()).is_err());
        assert_eq!(&candidate, players);
    }
    // A real alternative recipe declaration, with a separately fingerprinted effect.
    let mut changed_recipe = recipe();
    changed_recipe.consumables[0].oxygen += 1;
    let mut stored: StoredPlayers = serde_json::from_slice(&serde_json::to_vec(players)?)?;
    stored.context.inputs.declaration = fingerprint(&(
        "authored-consumable-recovery-rules-v1",
        fixture_declaration(),
        rates(),
        HZ,
        changed_recipe,
    ))?;
    let changed: PlayerSnapshot = serde_json::from_slice(&serde_json::to_vec(&stored)?)?;
    let effect_context = PlayerContext::new(
        "authored-consumable-recovery".encode_utf16().collect(),
        "/Authored/ConsumableRecovery".into(),
        PlayerInputs {
            declaration: stored.context.inputs.declaration,
            authored_gameplay: stored.context.inputs.authored_gameplay,
            spawn: stored.context.inputs.spawn,
            authored_world: stored.context.inputs.authored_world,
            initial_values: stored.context.inputs.initial_values,
        },
        vec!["AuthoredGrowth".into()],
    )?;
    let mut candidate = players.clone();
    assert!(candidate.open(&effect_context, &release()).is_err());
    assert_eq!(&candidate, players);
    assert_ne!(changed, *players);
    assert!(super::super::open_app(&snapshot).is_err());
    assert!(open_app(&super::super::expected(12)?).is_err());
    assert!(open_app(&crate::expected(12)?).is_err());
    Ok(())
}

#[test]
fn independently_mutated_targets_and_half_uses_are_rejected() -> Result<()> {
    let good = expected(12)?;
    for index in 0..10 {
        let mut bad = good.clone();
        mutate_players(&mut bad, |p| {
            p.records[0].vitals[index] += if index.is_multiple_of(2) { -1 } else { 1 }
        })?;
        assert!(check(&bad).is_err(), "vital {index}");
    }
    for field in 0..7 {
        let mut bad = good.clone();
        let inv = bad.player_inventory.as_mut().unwrap().as_v1_mut().unwrap();
        let icarus_bevy::native_player_inventory::PlayerInventoryRecipe::V2(recipe) =
            &mut inv.recipe
        else {
            unreachable!()
        };
        match field {
            0 => recipe.consumables[0].oxygen += 1,
            1 => recipe.consumables[0].water += 1,
            2 => recipe.consumables[0].food += 1,
            3 => recipe.items[0].stack_limit += 1,
            4 => recipe.format = 1,
            5 => inv.format = 2,
            _ => inv.bags[0].consumed[0].count = 1,
        }
        assert!(check(&bad).is_err());
    }
    let mut bad = good.clone();
    mutate_players(&mut bad, |p| p.records[0].growth_row.row.push('x'))?;
    assert!(check(&bad).is_err());
    for index in 0..5 {
        let mut bad = good.clone();
        mutate_players(&mut bad, |p| match index {
            0 => p.context.inputs.authored_gameplay[0] ^= 1,
            1 => p.context.inputs.spawn[0] ^= 1,
            2 => p.context.inputs.authored_world[0] ^= 1,
            3 => p.context.inputs.declaration[0] ^= 1,
            _ => p.context.inputs.initial_values[0] ^= 1,
        })?;
        assert!(check(&bad).is_err());
    }
    for doubled in [false, true] {
        let mut bad = good.clone();
        let inv = bad.player_inventory.as_mut().unwrap().as_v1_mut().unwrap();
        let bag = &mut inv.bags[0];
        let delta = if doubled { 1 } else { -1 };
        bag.inventories[0].slots[0].as_mut().unwrap().count =
            (i64::from(bag.inventories[0].slots[0].as_ref().unwrap().count) - delta) as u32;
        bag.consumed[1].count = (bag.consumed[1].count as i64 + delta) as u64;
        bad.validate()?;
        assert!(check(&bad).is_err(), "quantity-only half Use");
    }
    let mut bad = expected(11)?;
    mutate_players(&mut bad, |p| {
        p.records[0].vitals[4] = 1_000_000;
        p.records[0].vitals[6] = 1_000_000;
        p.records[0].vitals[8] = 1_000_000;
    })?;
    assert!(check(&bad).is_err(), "vital-only half Use");
    let mut bad = good.clone();
    let inv = bad.player_inventory.as_mut().unwrap().as_v1_mut().unwrap();
    inv.bags[0].inventories[0].slots[0].as_mut().unwrap().count += 1;
    inv.bags[1].inventories[0].slots[0].as_mut().unwrap().count -= 1;
    assert!(
        check_conservation(&bad).is_err(),
        "global totals cannot mask owner-local loss"
    );
    Ok(())
}

#[test]
fn complete_history_two_crash_gaps_and_lost_ack_selection_bounds() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = SnapshotStore::open(directory.path())?;
    let mut acks = Vec::new();
    let mut all = Vec::new();
    for (index, tick) in [0, 5, 5, 11, 12, 13].into_iter().enumerate() {
        let snapshot = expected(tick)?;
        let receipt = store.save(&snapshot)?;
        all.push(Acknowledgement {
            generation: receipt.generation,
            tick,
            digest: check(&snapshot)?,
        });
        if ![1, 3].contains(&index) {
            acks.push(Acknowledgement {
                generation: receipt.generation,
                tick,
                digest: check(&snapshot)?,
            });
        }
    }
    drop(store);
    // Full history includes distinct generations 2/3 with the identical tick 5.
    assert_eq!(
        verify(InventoryMode::Consumables, directory.path(), &all)?["acknowledgements"],
        6
    );
    let mut decreasing_tick = all.clone();
    decreasing_tick[2].tick = 4;
    decreasing_tick[2].digest = check(&expected(4)?)?;
    assert!(
        verify(
            InventoryMode::Consumables,
            directory.path(),
            &decreasing_tick
        )
        .is_err()
    );

    assert_eq!(
        verify(InventoryMode::Consumables, directory.path(), &acks)?["generation"],
        6
    );
    // Latest can be committed without its ACK, but two outstanding generations fail.
    assert_eq!(
        verify(InventoryMode::Consumables, directory.path(), &acks[..3])?["generation"],
        6
    );
    assert!(verify(InventoryMode::Consumables, directory.path(), &acks[..2]).is_err());
    assert!(verify(InventoryMode::Consumables, directory.path(), &[]).is_err());
    for index in 0..acks.len() {
        let mut bad = acks.clone();
        bad[index].digest = "0".repeat(64);
        assert!(verify(InventoryMode::Consumables, directory.path(), &bad).is_err());
    }
    let mut bad = acks.clone();
    bad.insert(1, bad[0].clone());
    assert!(verify(InventoryMode::Consumables, directory.path(), &bad).is_err());
    bad = acks.clone();
    bad.swap(0, 1);
    assert!(verify(InventoryMode::Consumables, directory.path(), &bad).is_err());
    for ack in [
        Acknowledgement {
            generation: 7,
            ..acks[3].clone()
        },
        Acknowledgement {
            tick: 14,
            ..acks[3].clone()
        },
    ] {
        assert!(verify(InventoryMode::Consumables, directory.path(), &[ack]).is_err());
    }
    fs::write(
        directory.path().join("snapshot-6.json"),
        b"authored corruption",
    )?;
    assert!(verify(InventoryMode::Consumables, directory.path(), &acks).is_err());
    Ok(())
}

#[test]
fn run_and_verify_reserve_eight_ticks_and_refuse_invalid_bounds_before_store() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let opt = options(&directory.path().join("missing"));
    let stopping = AtomicBool::new(true);
    for (duration, save) in [(2, 1000), (13, 1000), (3, 99), (3, 5001)] {
        assert!(
            run(InventoryMode::Consumables, &opt, duration, save, &stopping)
                .unwrap_err()
                .to_string()
                .contains("bounds")
        );
    }
    assert!(!opt.data_dir.exists());
    for tick in [MAX_TICK - 8, MAX_TICK - 7] {
        let directory = tempfile::tempdir()?;
        let snapshot = expected(tick)?;
        let ack = {
            let receipt = SnapshotStore::open(directory.path())?.save_new(&snapshot)?;
            Acknowledgement {
                generation: receipt.generation,
                tick,
                digest: check(&snapshot)?,
            }
        };
        assert_eq!(
            verify(InventoryMode::Consumables, directory.path(), &[ack]).is_ok(),
            tick == MAX_TICK - 8
        );
        if tick == MAX_TICK - 7 {
            assert!(
                run(
                    InventoryMode::Consumables,
                    &options(directory.path()),
                    3,
                    1000,
                    &stopping
                )
                .unwrap_err()
                .to_string()
                .contains("headroom")
            );
        }
    }
    Ok(())
}

#[test]
fn equal_tick_wrong_bag_submission_cannot_settle_original_cutoff() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let frozen = expected(5)?;
    let mut original = open_app(&frozen)?;
    let revision = player_revision(5);
    let now = Instant::now();
    original
        .world_mut()
        .resource_mut::<PlayerRegistry>()
        .request_leave(key(A)?, A, revision, now, Duration::from_secs(3))?;
    let mut wrong = frozen.clone();
    let bag = &mut wrong
        .player_inventory
        .as_mut()
        .unwrap()
        .as_v1_mut()
        .unwrap()
        .bags[0];
    bag.inventories[0].slots[0].as_mut().unwrap().count -= 1;
    bag.consumed[1].count += 1;
    wrong.validate()?;
    let mut other = build_app(&fixture_config(), &wrong)?;
    other.finish();
    other.cleanup();
    other
        .world_mut()
        .resource_mut::<PlayerRegistry>()
        .reserve(key(A)?, A, initial())?;
    activate_player(other.world_mut(), A)?;
    other
        .world_mut()
        .resource_mut::<PlayerRegistry>()
        .request_leave(key(A)?, A, revision, now, Duration::from_secs(3))?;
    let mut writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path())?)?;
    writer.submit(other.world_mut())?;
    writer
        .finish_before(original.world_mut(), now + Duration::from_secs(3))?
        .context("receipt")?;
    assert_eq!(leave_phase(&original)?, LeavePhase::Pending);
    assert!(
        original
            .world_mut()
            .resource_mut::<PlayerRegistry>()
            .settle_leave(key(A)?, A)
            .is_err()
    );
    assert_eq!(capture(original.world_mut())?, frozen);
    Ok(())
}

#[test]
fn storage_and_capture_failures_never_complete_leave() -> Result<()> {
    for capture_failure in [false, true] {
        let directory = tempfile::tempdir()?;
        let mut app = open_app(&expected(5)?)?;
        let mut writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path())?)?;
        let now = Instant::now();
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .request_leave(key(A)?, A, player_revision(5), now, Duration::from_secs(3))?;
        if capture_failure {
            app.world_mut().remove_resource::<SimulationClock>();
            assert!(writer.submit(app.world_mut()).is_err());
        } else {
            fs::create_dir(directory.path().join("snapshot-1.json"))?;
            writer.submit(app.world_mut())?;
            assert!(
                writer
                    .finish_before(app.world_mut(), now + Duration::from_secs(3))
                    .is_err()
            );
            assert_eq!(capture(app.world_mut())?, expected(5)?);
        }
        assert!(matches!(leave_phase(&app)?, LeavePhase::Failed(_)));
        assert!(
            app.world_mut()
                .resource_mut::<PlayerRegistry>()
                .settle_leave(key(A)?, A)
                .is_err()
        );
    }
    Ok(())
}

#[test]
fn receipt_input_bounds_and_strict_types_refuse_before_store() -> Result<()> {
    for input in [
        r#"[{"generation":true,"tick":0,"digest":"x"}]"#,
        r#"[{"generation":1,"tick":false,"digest":"x"}]"#,
        r#"[{"generation":1,"tick":0,"digest":7}]"#,
        r#"[{"generation":1,"tick":0,"digest":"x","extra":1}]"#,
        r#"[{"generation":1,"tick":0,"digest":"x"}"#,
    ] {
        assert!(serde_json::from_str::<Vec<Acknowledgement>>(input).is_err());
    }
    let directory = tempfile::tempdir()?;
    let path = directory.path().join("receipts.json");
    fs::write(&path, vec![b' '; 1024 * 1024 + 1])?;
    let options = Options {
        data_dir: directory.path().join("missing"),
        run_id: "consumable-unit".into(),
        command: Command::Verify { receipts: path },
    };
    assert!(
        execute(
            &options,
            &AtomicBool::new(false),
            InventoryMode::Consumables
        )
        .unwrap_err()
        .to_string()
        .contains("receipt input exceeds bound")
    );
    assert!(!options.data_dir.exists());
    Ok(())
}
