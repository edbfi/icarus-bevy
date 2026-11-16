use super::*;

fn mutate_players(
    snapshot: &mut WorldSnapshot,
    mutation: impl FnOnce(&mut StoredPlayers),
) -> Result<()> {
    let mut players: StoredPlayers = serde_json::from_slice(&serde_json::to_vec(
        snapshot.native_players.as_ref().context("players")?,
    )?)?;
    mutation(&mut players);
    snapshot.native_players = Some(serde_json::from_slice(&serde_json::to_vec(&players)?)?);
    Ok(())
}
fn options(directory: &Path) -> Options {
    Options {
        data_dir: directory.into(),
        run_id: "depletion-unit".into(),
        command: Command::Init,
    }
}
fn deadline() -> Instant {
    Instant::now() + Duration::from_secs(3)
}
fn leave(app: &mut App) -> Result<()> {
    let revision = app
        .world()
        .resource::<PlayerRegistry>()
        .projection(A)
        .context("A")?
        .revision;
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .request_leave(key(A)?, A, revision, Instant::now(), Duration::from_secs(3))?;
    Ok(())
}

#[test]
fn distinct_protocol_and_pairwise_cli_conflicts() -> Result<()> {
    assert_eq!(
        InventoryMode::Depletion.protocol(),
        "authored_player_depletion_recovery_v1"
    );
    let prefix = ["fixture", "--data-dir", "unused", "--run-id", "test"];
    for flags in [
        ["--inventory", "--consumables"],
        ["--inventory", "--depletion"],
        ["--consumables", "--depletion"],
    ] {
        for pair in [flags, [flags[1], flags[0]]] {
            assert!(
                Invocation::try_parse_from(prefix.into_iter().chain(pair).chain(["init"])).is_err()
            );
        }
    }
    let parsed = Invocation::try_parse_from(prefix.into_iter().chain(["--depletion", "init"]))?;
    assert!(parsed.depletion && !parsed.inventory && !parsed.consumables);
    let parsed = Invocation::try_parse_from(prefix.into_iter().chain(["init"]))?;
    assert!(!parsed.depletion && !parsed.inventory && !parsed.consumables);
    Ok(())
}

#[test]
fn literal_health_needs_revisions_and_two_day_periods() -> Result<()> {
    assert_eq!(seed()?, expected(0)?);
    let oxygen = [0, 0, 0, 0, 1, 1, 1, 2];
    let water = [0, 0, 0, 1, 1, 2, 2, 3];
    let food = [0, 0, 0, 0, 0, 1, 2, 3];
    let loss = [0, 0, 0, 1, 2, 4, 5, 8];
    for t in 0..=7 {
        assert_eq!(oxygen[t] + water[t] + food[t], loss[t]);
        assert_eq!(health(t as u64), 1_000_000 - loss[t]);
    }
    for (t, h, u) in [
        (3, 999_999, 0),
        (4, 999_998, 0),
        (6, 999_995, 0),
        (7, 999_992, 0),
        (8, 999_992, 1),
        (10, 999_991, 1),
        (14, 999_984, 1),
        (15, 999_984, 2),
    ] {
        assert_eq!((health(t), uses(t)), (h, u));
    }
    let mut app = open_app(&seed()?)?;
    for tick in 0..=154 {
        let snapshot = capture(app.world_mut())?;
        assert_eq!(snapshot, expected(tick)?);
        assert_eq!(
            app.world().resource::<PlayerRegistry>().activated_epochs(),
            vec![A]
        );
        let inv = snapshot.player_inventory.as_ref().unwrap();
        assert_eq!(
            (
                inv.revision,
                inv.bags[0].revision,
                inv.bags[1].revision,
                inv.next_stack_id
            ),
            (3 + uses(tick), 1 + uses(tick), 1, 5)
        );
        // Independent single-rounding day witness, including the wrap and +zero.
        let minute = ((36_960 + 1440 * (tick % 77)) % 110_880) as f64 / 77.;
        assert_eq!(consumables::check_day(tick)?, (minute as f32).to_bits());
        if tick < 154 {
            step(&mut app)?;
        }
    }
    assert_eq!(
        AuthoredClockConfig {
            initial_minute: 0,
            ..clock()
        }
        .prepare()?
        .sample(0)
        .to_bits(),
        0f32.to_bits()
    );
    assert!(health(MAX_TICK) > 885_000);
    expected(MAX_TICK)?.validate()?;
    assert!(expected(MAX_TICK + 1).is_err());
    let mut app = open_app(&expected(MAX_TICK)?)?;
    assert!(step(&mut app).is_err());
    assert_eq!(capture(app.world_mut())?, expected(MAX_TICK)?);
    Ok(())
}

#[test]
fn reopen_every_fractional_phase_and_verify_exact_witnesses_without_writes() -> Result<()> {
    for tick in (0..=14).chain([36, 76, 77, 78]) {
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
        let stored = fs::read(directory.path().join("snapshot-1.json"))?;
        let result = verify(InventoryMode::Depletion, directory.path(), &[ack])?;
        for (field, value) in [
            ("tick", tick),
            ("resumed_tick", tick + 8),
            ("uses", uses(tick)),
            ("resumed_uses", uses(tick + 8)),
            ("health", health(tick) as u64),
            ("resumed_health", health(tick + 8) as u64),
            ("day_sample_bits", consumables::check_day(tick)? as u64),
            (
                "resumed_day_sample_bits",
                consumables::check_day(tick + 8)? as u64,
            ),
        ] {
            assert_eq!(result[field], value, "{field} at {tick}");
        }
        assert_eq!(fs::read(directory.path().join("manifest.json"))?, manifest);
        assert_eq!(fs::read(directory.path().join("snapshot-1.json"))?, stored);
        let mut app = open_app(&snapshot)?;
        for next in tick + 1..=tick + 8 {
            step(&mut app)?;
            assert_eq!(capture(app.world_mut())?, expected(next)?);
        }
    }
    Ok(())
}

#[test]
fn use_preserves_health_and_full_state_cas_and_detached_refusals() -> Result<()> {
    let mut app = open_app(&expected(7)?)?;
    use_checked(&mut app)?; // Checks exact delta, stale player/bag CAS and full NoChange.
    let snapshot = capture(app.world_mut())?;
    let v = snapshot
        .native_players
        .as_ref()
        .unwrap()
        .projection_by_id(A)
        .unwrap()
        .checkpoint
        .vitals;
    assert_eq!((v.health, v.oxygen, v.water, v.food), (999_992, 1, 2, 3));
    let detached = request(
        &mut app,
        B,
        PlayerInventoryActionV1::Use {
            source: endpoint(B, 3)?,
        },
    )?;
    refused(&mut app, key(B)?, B, &detached)?;
    app.world_mut().run_schedule(FixedUpdate);
    assert_eq!(capture(app.world_mut())?, expected(8)?);
    Ok(())
}

#[test]
fn full_oracle_rejects_health_needs_context_recipe_and_half_uses() -> Result<()> {
    let good = expected(8)?;
    // Check each record independently, including the detached player and maxima.
    for owner in 0..2 {
        for vital in 0usize..10 {
            let mut bad = good.clone();
            mutate_players(&mut bad, |p| {
                if vital.is_multiple_of(2) {
                    let v = &mut p.records[owner].vitals;
                    v[vital] += if v[vital] < v[vital + 1] { 1 } else { -1 };
                } else {
                    p.records[owner].vitals[vital] += 1;
                }
            })?;
            assert!(check(&bad).is_err(), "owner {owner} vital {vital}");
        }
        let mut bad = good.clone();
        mutate_players(&mut bad, |p| p.records[owner].revision += 1)?;
        assert!(check(&bad).is_err());
    }
    for field in 0..9 {
        let mut bad = good.clone();
        mutate_players(&mut bad, |p| match field {
            0 => p.context.inputs.authored_gameplay[0] ^= 1,
            1 => p.context.inputs.spawn[0] ^= 1,
            2 => p.context.inputs.authored_world[0] ^= 1,
            3 => p.context.inputs.declaration[0] ^= 1,
            4 => p.context.inputs.initial_values[0] ^= 1,
            5 => p.high_water += 1,
            6 => p.records[0].location_cm[0] += 1.,
            7 => p.written_under.as_mut().unwrap().build_id += 1,
            _ => p.context.map_url.push('x'),
        })?;
        assert!(check(&bad).is_err());
    }
    for field in 0..9 {
        let mut bad = good.clone();
        let inv = bad.player_inventory.as_mut().unwrap();
        match field {
            0 => inv.revision += 1,
            1 => inv.bags[0].revision += 1,
            2 => inv.next_stack_id += 1,
            3 => inv.bags[0].consumed[1].count += 1,
            4 => inv.bags[0].inventories[0].slots[0].as_mut().unwrap().count -= 1,
            5 => inv.bags[0].consumed[0].count += 1,
            6 => inv.bags[0].inventories[0].overflow[0].stack.count -= 1,
            7 => inv.recipe_id = "0".repeat(64),
            _ => {
                let icarus_bevy::native_player_inventory::PlayerInventoryRecipe::V2(recipe) =
                    &mut inv.recipe
                else {
                    unreachable!()
                };
                recipe.consumables[0].oxygen += 1;
            }
        }
        assert!(check(&bad).is_err());
    }
    // Quantity-only and needs-only halves remain invalid even when conserved.
    let mut bad = good.clone();
    let inv = bad.player_inventory.as_mut().unwrap();
    inv.bags[0].consumed[1].count = 0;
    inv.bags[0].inventories[0].slots[0].as_mut().unwrap().count = 100_000;
    bad.validate()?;
    assert!(check(&bad).is_err());
    let mut bad = expected(7)?;
    mutate_players(&mut bad, |p| {
        p.records[0].vitals[4] = 1;
        p.records[0].vitals[6] = 2;
        p.records[0].vitals[8] = 3;
    })?;
    assert!(check(&bad).is_err());
    // A reset fractional phase has identical needs at tick 4, but different health.
    let mut bad = expected(4)?;
    mutate_players(&mut bad, |p| p.records[0].vitals[0] = 999_999)?;
    assert!(check(&bad).is_err());
    let mut bad = good.clone();
    bad.tick += 1;
    assert!(check(&bad).is_err());
    Ok(())
}

#[test]
fn reopen_binds_damage_rates_hz_clock_recipe_and_initial_values() -> Result<()> {
    let snapshot = expected(12)?;
    let original = snapshot.native_players.as_ref().unwrap();
    for different in [
        context_with_rules(
            ConsumptionConfig {
                oxygen: 8,
                ..consumption()
            },
            damage(),
            HZ,
            clock(),
        )?,
        context_with_rules(
            consumption(),
            DepletionDamageConfig {
                food: 6,
                ..damage()
            },
            HZ,
            clock(),
        )?,
        context_with_rules(consumption(), damage(), 8, clock())?,
        context_with_rules(
            consumption(),
            damage(),
            HZ,
            AuthoredClockConfig {
                seconds_per_day: 12,
                ..clock()
            },
        )?,
        context_with_rules(
            consumption(),
            damage(),
            HZ,
            AuthoredClockConfig {
                initial_minute: 481,
                ..clock()
            },
        )?,
    ] {
        let mut candidate = original.clone();
        assert!(candidate.open(&different, &release()).is_err());
        assert_eq!(&candidate, original);
    }
    for field in 0..3 {
        let mut bad = snapshot.clone();
        mutate_players(&mut bad, |p| match field {
            0 => p.context.inputs.initial_values[0] ^= 1,
            1 => p.context.inputs.authored_gameplay[0] ^= 1,
            _ => p.context.prospect_id.push(65),
        })?;
        let unchanged = bad.clone();
        assert!(open_app(&bad).is_err());
        assert_eq!(bad, unchanged);
    }
    assert!(open_app(&consumables::expected(12)?).is_err());
    assert!(consumables::open_app(&snapshot).is_err());
    assert!(super::super::open_app(&snapshot).is_err());
    Ok(())
}

#[test]
fn damage_clamps_at_zero_and_newly_empty_needs_wait_until_next_batch() -> Result<()> {
    // Short independent fixture: positive food delta at tick 3 must be excluded
    // because food was nonzero at the cut, while water contributes one.
    for (start, after_three, after_four, revision) in [(1, 0, 0, 4), (5, 4, 3, 5)] {
        let mut short = expected(2)?;
        mutate_players(&mut short, |p| p.records[0].vitals[0] = start)?;
        let mut app = build_app(&fixture_config(), &short)?;
        app.finish();
        app.cleanup();
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .reserve(key(A)?, A, checkpoint())?;
        activate_player(app.world_mut(), A)?;
        app.world_mut().run_schedule(FixedUpdate);
        let snapshot = capture(app.world_mut())?;
        let a = snapshot
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(A)
            .unwrap();
        assert_eq!(
            (
                a.checkpoint.vitals.health,
                a.checkpoint.vitals.food,
                a.revision
            ),
            (after_three, 0, 4)
        );
        app.world_mut().run_schedule(FixedUpdate);
        let snapshot = capture(app.world_mut())?;
        let a = snapshot
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(A)
            .unwrap();
        assert_eq!(
            (a.checkpoint.vitals.health, a.revision),
            (after_four, revision)
        );
        assert!(check(&snapshot).is_err());
    }
    Ok(())
}

#[test]
fn wrong_receipt_or_pending_digest_preserves_ack_state() -> Result<()> {
    let directory = tempfile::tempdir()?;
    for (generation, tick, wrong_digest) in [(2, 7, false), (1, 8, false), (1, 7, true)] {
        let mut pending = Some(Acknowledgement {
            generation: 1,
            tick: 7,
            digest: if wrong_digest {
                "0".repeat(64)
            } else {
                check(&expected(7)?)?
            },
        });
        let before = serde_json::to_vec(&pending)?;
        let mut current = 0;
        assert!(
            acknowledge(
                InventoryMode::Depletion,
                &options(directory.path()),
                SaveReceipt { generation, tick },
                &mut pending,
                &mut current
            )
            .is_err()
        );
        assert_eq!(current, 0);
        assert_eq!(serde_json::to_vec(&pending)?, before);
    }
    Ok(())
}

#[test]
fn older_health_submission_cannot_settle_new_frozen_leave_cutoff() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = open_app(&expected(7)?)?;
    let mut writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path())?)?;
    let mut pending = None;
    submit(
        InventoryMode::Depletion,
        &mut app,
        &mut writer,
        0,
        &mut pending,
        None,
    )?;
    for _ in 0..3 {
        step(&mut app)?;
    }
    let frozen = capture(app.world_mut())?;
    assert!(health(10) < health(7));
    leave(&mut app)?;
    let old = writer
        .finish_before(app.world_mut(), deadline())?
        .context("old receipt")?;
    matched(InventoryMode::Depletion, &old, &pending, 0)?;
    assert_eq!(leave_phase(&app)?, LeavePhase::Pending);
    assert!(
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .settle_leave(key(A)?, A)
            .is_err()
    );
    pending = None;
    submit(
        InventoryMode::Depletion,
        &mut app,
        &mut writer,
        1,
        &mut pending,
        Some(deadline()),
    )?;
    let receipt = writer
        .finish_before(app.world_mut(), deadline())?
        .context("final receipt")?;
    matched(InventoryMode::Depletion, &receipt, &pending, 1)?;
    assert_eq!(
        leave_phase(&app)?,
        LeavePhase::Durable {
            generation: 2,
            tick: 10
        }
    );
    assert_eq!(capture(app.world_mut())?, frozen);
    {
        let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
        assert!(registry.detach(A));
        registry.settle_leave(key(A)?, A)?;
        assert_eq!(registry.leave_counts().durable_unsettled, 0);
    }
    assert_eq!(capture(app.world_mut())?, frozen);
    drop(writer);
    assert_eq!(
        SnapshotStore::open_existing(directory.path())?
            .load()?
            .unwrap()
            .snapshot,
        frozen
    );
    Ok(())
}

#[test]
fn equal_tick_different_health_receipt_cannot_settle_original_leave() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let frozen = expected(7)?;
    let mut original = open_app(&frozen)?;
    leave(&mut original)?;
    let mut wrong = frozen.clone();
    mutate_players(&mut wrong, |p| p.records[0].vitals[0] -= 1)?;
    wrong.validate()?;
    assert_ne!(digest(&serde_json::to_vec(&wrong)?), check(&frozen)?);
    let mut other = build_app(&fixture_config(), &wrong)?;
    other.finish();
    other.cleanup();
    other
        .world_mut()
        .resource_mut::<PlayerRegistry>()
        .reserve(key(A)?, A, checkpoint())?;
    activate_player(other.world_mut(), A)?;
    leave(&mut other)?;
    let mut writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path())?)?;
    writer.submit(other.world_mut())?;
    let receipt = writer
        .finish_before(original.world_mut(), deadline())?
        .context("receipt")?;
    assert_eq!(receipt.tick, frozen.tick);
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
fn shared_shutdown_checks_old_receipt_before_completing_frozen_health() -> Result<()> {
    for corrupt in [false, true] {
        let directory = tempfile::tempdir()?;
        let mut app = open_app(&expected(7)?)?;
        let mut writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path())?)?;
        let mut pending = None;
        let mut generation = 0;
        submit(
            InventoryMode::Depletion,
            &mut app,
            &mut writer,
            generation,
            &mut pending,
            None,
        )?;
        for _ in 0..3 {
            step(&mut app)?;
        }
        let frozen = capture(app.world_mut())?;
        if corrupt {
            pending.as_mut().unwrap().digest = "0".repeat(64);
        }
        let result = shutdown(
            InventoryMode::Depletion,
            &options(directory.path()),
            &mut app,
            &mut writer,
            &mut generation,
            &mut pending,
        );
        assert_eq!(capture(app.world_mut())?, frozen);
        if corrupt {
            assert!(result.is_err());
            assert_eq!(generation, 0);
            assert!(pending.is_some());
            assert_eq!(leave_phase(&app)?, LeavePhase::Pending);
        } else {
            result?;
            assert_eq!(generation, 2);
            assert!(pending.is_none());
            assert_eq!(
                app.world()
                    .resource::<PlayerRegistry>()
                    .leave_counts()
                    .detached_settled,
                1
            );
        }
    }
    Ok(())
}

#[test]
fn storage_capture_and_expired_deadline_fail_without_settlement() -> Result<()> {
    for failure in 0..3 {
        let directory = tempfile::tempdir()?;
        let mut app = open_app(&expected(7)?)?;
        let mut writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path())?)?;
        leave(&mut app)?;
        match failure {
            0 => {
                app.world_mut().remove_resource::<SimulationClock>();
                assert!(writer.submit(app.world_mut()).is_err());
            }
            1 => {
                fs::create_dir(directory.path().join("snapshot-1.json"))?;
                writer.submit(app.world_mut())?;
                assert!(writer.finish_before(app.world_mut(), deadline()).is_err());
            }
            _ => {
                let mut pending = None;
                assert!(
                    submit(
                        InventoryMode::Depletion,
                        &mut app,
                        &mut writer,
                        0,
                        &mut pending,
                        Some(Instant::now())
                    )
                    .is_err()
                );
                assert!(pending.is_none());
            }
        }
        assert!(!matches!(leave_phase(&app)?, LeavePhase::Durable { .. }));
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
fn historical_ack_health_digests_and_single_pending_selection_are_exact() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut history = Vec::new();
    {
        let mut store = SnapshotStore::open(directory.path())?;
        for tick in [7, 10, 14] {
            let snapshot = expected(tick)?;
            let receipt = store.save(&snapshot)?;
            history.push(Acknowledgement {
                generation: receipt.generation,
                tick,
                digest: check(&snapshot)?,
            });
        }
    }
    let mode = InventoryMode::Depletion;
    assert_eq!(verify(mode, directory.path(), &history)?["generation"], 3);
    // One committed generation may have lost its ACK, but never two.
    assert_eq!(verify(mode, directory.path(), &history[..2])?["tick"], 14);
    assert!(verify(mode, directory.path(), &history[..1]).is_err());
    let mut wrong = expected(7)?;
    mutate_players(&mut wrong, |p| p.records[0].vitals[0] -= 1)?;
    let mut bad_history = history.clone();
    bad_history[0].digest = digest(&serde_json::to_vec(&wrong)?);
    assert!(verify(mode, directory.path(), &bad_history).is_err());
    let mut bad_history = history.clone();
    bad_history[2].digest = history[1].digest.clone();
    assert!(verify(mode, directory.path(), &bad_history).is_err());
    assert!(verify(mode, directory.path(), &[]).is_err());
    Ok(())
}

#[test]
fn verifier_reserves_eight_steps_and_fractional_phase_reset_refuses() -> Result<()> {
    for tick in [MAX_TICK - 8, MAX_TICK - 7] {
        let directory = tempfile::tempdir()?;
        let snapshot = expected(tick)?;
        let receipt = SnapshotStore::open(directory.path())?.save_new(&snapshot)?;
        let ack = Acknowledgement {
            generation: receipt.generation,
            tick,
            digest: check(&snapshot)?,
        };
        let manifest = fs::read(directory.path().join("manifest.json"))?;
        let result = verify(InventoryMode::Depletion, directory.path(), &[ack]);
        if tick == MAX_TICK - 8 {
            assert_eq!(result?["resumed_tick"], MAX_TICK);
        } else {
            assert!(result.unwrap_err().to_string().contains("headroom"));
        }
        assert_eq!(fs::read(directory.path().join("manifest.json"))?, manifest);
    }
    let mut app = open_app(&expected(4)?)?;
    app.world_mut().resource_mut::<SimulationClock>().tick = 0;
    assert!(step(&mut app).is_err());
    assert!(check(&capture(app.world_mut())?).is_err());
    Ok(())
}
