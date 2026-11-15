use super::*;

#[test]
fn fractional_reopen_and_detached_record_match_integer_oracle() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = open_app(&seed()?)?;
    for _ in 0..3 {
        step(&mut app)?;
    }
    let snapshot = capture(app.world_mut())?;
    assert_eq!(snapshot.tick, 3);
    let hash = check(&snapshot)?;
    let ack = {
        let mut store = SnapshotStore::open(directory.path())?;
        let receipt = store.save_new(&snapshot)?;
        Acknowledgement {
            generation: receipt.generation,
            tick: receipt.tick,
            digest: hash,
        }
    };
    let result = verify(directory.path(), &[ack])?;
    assert_eq!(result["resumed_tick"], 11);
    let mut reopened = open_app(&snapshot)?;
    assert_eq!(capture(reopened.world_mut())?, snapshot);
    for _ in 0..4 {
        step(&mut reopened)?;
    }
    check(&capture(reopened.world_mut())?)?;
    Ok(())
}

#[test]
fn typed_oracle_rejects_checkpoint_context_tick_and_receipt_mutations() -> Result<()> {
    let mut app = open_app(&seed()?)?;
    for _ in 0..9 {
        step(&mut app)?;
    }
    let good = capture(app.world_mut())?;
    let mut bad = good.clone();
    bad.tick += 1;
    assert!(check(&bad).is_err());
    let mut registry = PlayerRegistry::new(
        good.native_players.clone().context("players")?,
        &context()?,
        2,
    )?;
    let p = registry.snapshot().projection(key(1)?).context("player")?;
    let mut changed = p.checkpoint;
    changed.vitals.food -= 1;
    registry.edit(key(1)?, p.revision, changed)?;
    bad = good.clone();
    bad.native_players = Some(registry.snapshot());
    assert!(check(&bad).is_err());
    bad = good.clone();
    bad.native_players = Some(PlayerSnapshot::empty(context()?));
    assert!(check(&bad).is_err());
    let mut players = good.native_players.clone().context("players")?;
    let different = PlayerContext::new(
        "different".encode_utf16().collect(),
        "/Authored/Recovery".into(),
        PlayerInputs {
            authored_gameplay: [1; 32],
            spawn: [2; 32],
            authored_world: [3; 32],
            declaration: [4; 32],
            initial_values: [5; 32],
        },
        vec!["AuthoredGrowth".into()],
    )?;
    assert!(players.open(&different, &release()).is_err());
    let directory = tempfile::tempdir()?;
    {
        SnapshotStore::open(directory.path())?.save_new(&good)?;
    }
    let ack = Acknowledgement {
        generation: 1,
        tick: 9,
        digest: check(&good)?,
    };
    verify(directory.path(), std::slice::from_ref(&ack))?;
    assert!(verify(directory.path(), &[]).is_err());
    for wrong in [
        Acknowledgement {
            generation: 2,
            ..ack.clone()
        },
        Acknowledgement {
            tick: 8,
            ..ack.clone()
        },
        Acknowledgement {
            digest: "0".repeat(64),
            ..ack.clone()
        },
    ] {
        assert!(verify(directory.path(), &[wrong]).is_err());
    }
    assert!(verify(directory.path(), &[ack.clone(), ack]).is_err());
    Ok(())
}

#[test]
fn rule_rate_and_frequency_changes_refuse_the_committed_binding() -> Result<()> {
    let players = seed()?.native_players.context("players")?;
    for changed in [
        context_for_rules(
            ConsumptionConfig {
                oxygen: 3,
                ..rates()
            },
            HZ,
        )?,
        context_for_rules(rates(), HZ + 1)?,
    ] {
        let mut opened = players.clone();
        assert!(opened.open(&changed, &release()).is_err());
        assert_eq!(opened, players);
    }
    Ok(())
}

#[test]
fn lost_acknowledged_current_generation_is_denied_on_fallback() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let mut app = open_app(&seed()?)?;
    let ack = {
        let mut store = SnapshotStore::open(directory.path())?;
        store.save_new(&capture(app.world_mut())?)?;
        for _ in 0..3 {
            step(&mut app)?;
        }
        let snapshot = capture(app.world_mut())?;
        let receipt = store.save(&snapshot)?;
        Acknowledgement {
            generation: receipt.generation,
            tick: receipt.tick,
            digest: check(&snapshot)?,
        }
    };
    fs::write(
        directory.path().join("snapshot-2.json"),
        b"authored corruption fixture",
    )?;
    assert!(verify(directory.path(), &[ack]).is_err());
    Ok(())
}

#[test]
fn asynchronous_writer_receipt_must_match_exact_submitted_generation_and_tick() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let store = SnapshotStore::open(directory.path())?;
    let mut worker = SaveWorker::spawn(store)?;
    let mut app = open_app(&seed()?)?;
    for _ in 0..3 {
        step(&mut app)?;
    }
    let mut pending = None;
    submit(&mut app, &mut worker, 0, &mut pending)?;
    let receipt = worker
        .finish_before(Instant::now() + Duration::from_secs(3))?
        .context("receipt")?;
    let ack = pending.take().context("pending")?;
    assert_eq!(
        (receipt.generation, receipt.tick),
        (ack.generation, ack.tick)
    );
    assert_eq!(ack.digest, check(&capture(app.world_mut())?)?);
    let options = Options {
        data_dir: directory.path().into(),
        run_id: "authored-unit".into(),
        command: Command::Init,
    };
    let mut generation = 0;
    pending = Some(ack);
    let wrong = SaveReceipt {
        generation: receipt.generation,
        tick: receipt.tick + 1,
    };
    assert!(acknowledge(&options, wrong, &mut pending, &mut generation).is_err());
    assert_eq!(generation, 0);
    Ok(())
}
