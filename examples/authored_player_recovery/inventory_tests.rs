use super::*;

fn options(directory: &Path) -> Options {
    Options {
        data_dir: directory.into(),
        run_id: "inventory-unit".into(),
        command: Command::Init,
    }
}

#[test]
fn literal_seed_and_every_phase_restart_match_without_reissuance() -> Result<()> {
    let compiled = CompiledPlayerInventoryRecipe::compile(recipe())?;
    assert_eq!(compiled.identity(), recipe_identity()?);
    let seed = seed()?;
    assert_eq!(seed, expected(0)?);
    let mut app = open_app(&seed)?;
    for tick in 0..=17 {
        let snapshot = capture(app.world_mut())?;
        assert_eq!(snapshot.tick, tick);
        check(&snapshot)?;
        let mut reopened = open_app(&snapshot)?;
        for _ in 0..8 {
            step(&mut reopened)?;
        }
        assert_eq!(capture(reopened.world_mut())?, expected(tick + 8)?);
        if tick < 17 {
            step(&mut app)?;
        }
    }
    let final_state = capture(app.world_mut())?;
    let inventory = final_state.player_inventory.context("inventory")?;
    assert_eq!(inventory.next_stack_id, 8);
    assert!(
        inventory
            .bags
            .iter()
            .flat_map(|b| &b.inventories)
            .flat_map(|i| i.slots.iter().flatten())
            .all(|s| ![5, 6, 7].contains(&s.id.get()))
    );
    for tick in 0..=100 {
        let batches = (1..=tick)
            .filter(|t| [2, 3, 5].iter().any(|r| t * r / 7 != (t - 1) * r / 7))
            .count() as u64;
        assert_eq!(survival_batches(tick), batches);
    }
    expected(MAX_TICK)?.validate()?;
    assert!(expected(MAX_TICK + 1).is_err());
    let mut bound = open_app(&expected(MAX_TICK)?)?;
    let before = capture(bound.world_mut())?;
    assert!(step(&mut bound).is_err());
    assert_eq!(capture(bound.world_mut())?, before);
    Ok(())
}

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
                let $inv = bad.player_inventory.as_mut().unwrap();
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
        bad.player_inventory.as_mut().unwrap().next_stack_id += 1;
        bad.validate()?;
    });
    inventory_reject!(i, {
        i.recipe_id = "0".repeat(64);
    });
    inventory_reject!(i, {
        i.recipe.items[0].stack_limit += 1;
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
        let i = bad.player_inventory.as_mut().unwrap();
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

#[test]
fn context_binds_survival_checkpoint_and_day_inputs() -> Result<()> {
    let players = expected(0)?.native_players.context("players")?;
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
    ] {
        let mut candidate = players.clone();
        assert!(candidate.open(&different, &release()).is_err());
        assert_eq!(candidate, players);
    }
    Ok(())
}

#[test]
fn every_ack_and_lost_acknowledged_fallback_are_checked() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut store = SnapshotStore::open(directory.path())?;
    let mut acks = Vec::new();
    for tick in [0, 3, 9] {
        let snapshot = expected(tick)?;
        let receipt = store.save(&snapshot)?;
        acks.push(Acknowledgement {
            generation: receipt.generation,
            tick,
            digest: check(&snapshot)?,
        });
    }
    drop(store);
    assert_eq!(verify(directory.path(), &acks)?["resumed_tick"], 17);
    let mut bad = acks.clone();
    bad[0].digest = "0".repeat(64);
    assert!(verify(directory.path(), &bad).is_err());
    assert!(verify(directory.path(), &[]).is_err());
    fs::write(
        directory.path().join("snapshot-3.json"),
        b"authored corruption fixture",
    )?;
    assert!(verify(directory.path(), &acks).is_err());
    Ok(())
}

#[test]
fn exact_new_submission_is_required_for_same_tick_leave() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = open_app(&seed()?)?;
    for _ in 0..3 {
        step(&mut app)?;
    }
    let frozen = capture(app.world_mut())?;
    let mut writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path())?)?;
    let mut pending = None;
    submit(&mut app, &mut writer, 0, &mut pending, None)?;
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
        PlayerInventoryActionV1::Consume {
            source: endpoint(A, 1)?,
            amount: 1,
        },
    )?;
    refused(&mut app, key(A)?, A, &req)?;
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
    matched(&old, &pending, 0)?;
    assert_eq!(leave_phase(&app)?, LeavePhase::Pending);
    let generation = old.generation;
    pending = None;
    submit(
        &mut app,
        &mut writer,
        generation,
        &mut pending,
        Some(deadline),
    )?;
    let final_receipt = writer
        .finish_before(app.world_mut(), deadline)?
        .context("final receipt")?;
    matched(&final_receipt, &pending, generation)?;
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
    pending.as_mut().unwrap().digest = "0".repeat(64);
    assert!(
        acknowledge(
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
    assert!(submit(&mut app, &mut writer, 0, &mut pending, Some(now)).is_err());
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
        for _ in 0..9 {
            step(&mut app)?;
        }
        let frozen = capture(app.world_mut())?;
        let mut writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path())?)?;
        let mut generation = 0;
        let mut pending = None;
        submit(&mut app, &mut writer, generation, &mut pending, None)?;
        if corrupt {
            pending.as_mut().unwrap().tick += 1;
        }
        let result = shutdown(
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
fn explicit_mode_selection_retains_legacy_cli() {
    for inventory in [false, true] {
        for command in [
            vec!["init"],
            vec!["run"],
            vec!["verify", "--receipts", "receipts.json"],
        ] {
            let mut args = vec!["fixture", "--data-dir", "state", "--run-id", "unit"];
            if inventory {
                args.push("--inventory");
            }
            args.extend(command);
            assert_eq!(
                Invocation::try_parse_from(args).unwrap().inventory,
                inventory
            );
        }
    }
}
