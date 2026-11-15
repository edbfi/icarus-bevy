use super::*;
use crate::{
    config::ServerConfig,
    native_player_runtime::consumption_tests::{attach, registry},
    simulation::{SNAPSHOT_SCHEMA_PLAYERS, SimulationClock, build_app},
};
use bevy::{prelude::App, time::TimeUpdateStrategy};

fn key(account: u64) -> PlayerKey {
    PlayerKey::new(account, 3).unwrap()
}
fn app() -> App {
    let mut registry = registry();
    attach(&mut registry, 1, 1);
    attach(&mut registry, 2, 2);
    let snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYERS,
        native_players: Some(registry.snapshot()),
        ..Default::default()
    };
    let mut app = build_app(&ServerConfig::default(), &snapshot).unwrap();
    app.insert_resource(registry);
    app
}
fn request(app: &mut App, account: u64, epoch: u64) -> LeaveStatus {
    let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
    let revision = registry.projection(epoch).unwrap().revision;
    registry
        .request_leave(key(account), epoch, revision, Instant::now(), MAX_ATTEMPT)
        .unwrap()
}
fn phase(app: &App, account: u64, epoch: u64) -> LeavePhase {
    app.world()
        .resource::<PlayerRegistry>()
        .leave_status(key(account), epoch)
        .unwrap()
        .phase
}
fn writer() -> (tempfile::TempDir, CheckpointWriter) {
    let directory = tempfile::tempdir().unwrap();
    let writer = CheckpointWriter::spawn(SnapshotStore::open(directory.path()).unwrap()).unwrap();
    (directory, writer)
}
fn finish(writer: &mut CheckpointWriter, app: &mut App) -> SaveReceipt {
    writer
        .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(5))
        .unwrap()
        .unwrap()
}

#[test]
fn older_inflight_equal_tick_and_duplicate_cutoff_do_not_ack_later_leave() {
    let mut app = app();
    let (_directory, mut writer) = writer();
    writer.submit(app.world_mut()).unwrap();
    let first = request(&mut app, 1, 1);
    // A duplicate returns the same status even with a different timeout.
    assert_eq!(
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .request_leave(
                key(1),
                1,
                first.revision,
                Instant::now(),
                Duration::from_secs(1)
            )
            .unwrap(),
        first
    );
    let older = finish(&mut writer, &mut app);
    assert_eq!(phase(&app, 1, 1), LeavePhase::Pending);
    assert_eq!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_counts()
            .cutoffs_accepted,
        1
    );
    assert_eq!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_counts()
            .durable,
        0
    );
    assert!(
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .checkpoint_save_needed(Instant::now())
    );
    writer.submit(app.world_mut()).unwrap();
    // Second player arrived after capture: this receipt cannot cross-complete it.
    request(&mut app, 2, 2);
    let first_save = finish(&mut writer, &mut app);
    assert_eq!(older.tick, first_save.tick);
    assert_eq!(
        phase(&app, 1, 1),
        LeavePhase::Durable {
            generation: first_save.generation,
            tick: first_save.tick
        }
    );
    assert_eq!(phase(&app, 2, 2), LeavePhase::Pending);
    writer.submit(app.world_mut()).unwrap();
    let second_save = finish(&mut writer, &mut app);
    assert!(second_save.generation > first_save.generation);
    assert_eq!(
        phase(&app, 2, 2),
        LeavePhase::Durable {
            generation: second_save.generation,
            tick: second_save.tick
        }
    );
    // Polling completion repeatedly/backpressure cannot resubmit or undo it.
    for _ in 0..8 {
        assert!(writer.poll(app.world_mut()).unwrap().is_none());
    }
    assert!(
        !app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .checkpoint_save_needed(Instant::now())
    );
    let counts = app.world().resource::<PlayerRegistry>().leave_counts();
    assert_eq!(
        (
            counts.cutoffs_accepted,
            counts.durable,
            counts.durable_unsettled
        ),
        (2, 2, 2)
    );
    assert_eq!(
        counts.last_durable.unwrap().generation,
        second_save.generation
    );
}

#[test]
fn concurrent_leaves_batch_and_restart_exact_acknowledged_authored_state() {
    let mut app = app();
    let checkpoint = {
        let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
        let before = registry.projection(1).unwrap();
        let mut checkpoint = before.checkpoint;
        checkpoint.location_cm = [123., -456., 789.];
        checkpoint.vitals.health = 37;
        registry.edit(key(1), before.revision, checkpoint).unwrap();
        checkpoint
    };
    request(&mut app, 1, 1);
    request(&mut app, 2, 2);
    let expected = capture(app.world_mut()).unwrap();
    let (directory, mut writer) = writer();
    writer.submit(app.world_mut()).unwrap();
    let receipt = finish(&mut writer, &mut app);
    for (account, epoch) in [(1, 1), (2, 2)] {
        assert_eq!(
            phase(&app, account, epoch),
            LeavePhase::Durable {
                generation: receipt.generation,
                tick: receipt.tick
            }
        );
    }
    // Reads do not require the still-live worker's single writer lock.
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(directory.path().join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["current"]["number"], receipt.generation);
    // Drop the sender and wait boundedly for the worker to release its lock.
    drop(writer);
    let deadline = Instant::now() + Duration::from_secs(5);
    let store = loop {
        match SnapshotStore::open_existing(directory.path()) {
            Ok(store) => break store,
            Err(_) if Instant::now() < deadline => std::thread::yield_now(),
            Err(error) => panic!("writer did not exit: {error:#}"),
        }
    };
    let loaded = store.load().unwrap().unwrap().snapshot;
    assert_eq!(loaded, expected);
    assert_eq!(
        loaded
            .native_players
            .as_ref()
            .unwrap()
            .projection(key(1))
            .unwrap()
            .checkpoint,
        checkpoint
    );
    let mut restarted = build_app(&ServerConfig::default(), &loaded).unwrap();
    let mut registry = restarted.world_mut().resource_mut::<PlayerRegistry>();
    let projection = registry.reserve(key(1), 1, checkpoint).unwrap();
    assert_eq!(projection.revision, 2);
    assert_eq!(projection.checkpoint, checkpoint);
    assert!(!projection.is_new);
}

#[test]
fn zero_fixed_tick_cutoff_blocks_edit_debit_activation_and_reattach_until_settled() {
    let mut app = app();
    request(&mut app, 1, 1);
    let pinned = app
        .world()
        .resource::<PlayerRegistry>()
        .projection(1)
        .unwrap();
    app.insert_resource(TimeUpdateStrategy::ManualDuration(Duration::ZERO));
    app.update();
    assert_eq!(app.world().resource::<SimulationClock>().tick, 0);
    {
        let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
        assert_eq!(registry.activated_epochs(), vec![2]);
        assert!(registry.activate(1).is_err());
        let mut edit = pinned.checkpoint;
        edit.vitals.health = 2;
        assert!(registry.edit(key(1), pinned.revision, edit).is_err());
        registry
            .consume_activated(ConsumptionRates::new(1, 1, 1).unwrap())
            .unwrap();
        assert_eq!(registry.projection(1).unwrap(), pinned);
        registry.detach(1);
        assert!(registry.reserve(key(1), 3, pinned.checkpoint).is_err());
        assert!(registry.settle_leave(key(1), 1).is_err());
    }
    let (_directory, mut writer) = writer();
    writer.submit(app.world_mut()).unwrap();
    finish(&mut writer, &mut app);
    let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
    assert!(registry.reserve(key(1), 3, pinned.checkpoint).is_err());
    registry.settle_leave(key(1), 1).unwrap();
    registry.reserve(key(1), 3, pinned.checkpoint).unwrap();
    registry.activate(3).unwrap();
}

#[test]
fn bounded_disconnected_obligations_refuse_new_cutoff_without_mutating_it() {
    let mut registry = registry();
    for epoch in 1..=64 {
        attach(&mut registry, epoch, epoch);
        registry
            .request_leave(key(epoch), epoch, 1, Instant::now(), MAX_ATTEMPT)
            .unwrap();
        registry.detach(epoch);
    }
    attach(&mut registry, 100, 65);
    let before = registry.snapshot();
    assert_eq!(
        registry.request_leave(key(100), 65, 1, Instant::now(), MAX_ATTEMPT),
        Err(PlayerError::Limit)
    );
    assert_eq!(registry.snapshot(), before);
    assert_eq!(registry.activated_epochs(), vec![65]);
    assert_eq!(registry.leaves.len(), 64);
}

#[test]
fn busy_capture_fault_and_explicit_retry_preserve_cutoff() {
    let mut app = app();
    let (_directory, mut writer) = writer();
    writer.submit(app.world_mut()).unwrap();
    request(&mut app, 1, 1);
    assert!(writer.submit(app.world_mut()).is_err());
    assert_eq!(phase(&app, 1, 1), LeavePhase::Pending);
    finish(&mut writer, &mut app);
    let owner = app
        .world_mut()
        .remove_resource::<SimulationClock>()
        .unwrap();
    assert!(writer.submit(app.world_mut()).is_err());
    assert_eq!(phase(&app, 1, 1), LeavePhase::Failed(LeaveFailure::Capture));
    app.insert_resource(owner);
    // Periodic success is not explicit recovery of a failed operation.
    writer.submit(app.world_mut()).unwrap();
    finish(&mut writer, &mut app);
    assert_eq!(phase(&app, 1, 1), LeavePhase::Failed(LeaveFailure::Capture));
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .retry_leave(key(1), 1, Instant::now(), MAX_ATTEMPT)
        .unwrap();
    writer.submit(app.world_mut()).unwrap();
    finish(&mut writer, &mut app);
    assert!(matches!(phase(&app, 1, 1), LeavePhase::Durable { .. }));
    let counts = app.world().resource::<PlayerRegistry>().leave_counts();
    assert_eq!(
        (
            counts.cutoffs_accepted,
            counts.retries_accepted,
            counts.capture_failed,
            counts.durable
        ),
        (1, 1, 1, 1)
    );
}

#[test]
fn real_manifest_storage_failure_keeps_dirty_barrier_and_never_claims_success() {
    let mut app = app();
    request(&mut app, 1, 1);
    let (directory, mut writer) = writer();
    // A directory refuses atomic manifest replacement after generation sync.
    std::fs::create_dir(directory.path().join("manifest.json")).unwrap();
    writer.submit(app.world_mut()).unwrap();
    assert!(
        writer
            .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(5))
            .is_err()
    );
    assert_eq!(phase(&app, 1, 1), LeavePhase::Failed(LeaveFailure::Storage));
    let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
    let before = registry.snapshot();
    registry.detach(1);
    assert!(registry.settle_leave(key(1), 1).is_err());
    assert!(
        registry
            .reserve(key(1), 3, before.projection(key(1)).unwrap().checkpoint)
            .is_err()
    );
    assert_eq!(registry.snapshot(), before);
}

#[test]
fn timeout_cannot_cancel_write_or_allow_old_attempt_receipt_to_finish_retry() {
    let mut app = app();
    request(&mut app, 1, 1);
    let (_directory, mut writer) = writer();
    writer.submit(app.world_mut()).unwrap();
    assert!(
        writer
            .settle_before(app.world_mut(), Instant::now())
            .is_err()
    );
    assert_eq!(
        phase(&app, 1, 1),
        LeavePhase::Failed(LeaveFailure::Deadline)
    );
    assert!(
        writer.is_busy(),
        "deadline must not pretend the synchronous write was cancelled"
    );
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .retry_leave(key(1), 1, Instant::now(), MAX_ATTEMPT)
        .unwrap();
    finish(&mut writer, &mut app);
    assert_eq!(phase(&app, 1, 1), LeavePhase::Pending);
    // Separate request deadline expiry also suppresses a late successful result.
    let counts = app.world().resource::<PlayerRegistry>().leave_counts();
    assert_eq!(
        (
            counts.deadline_failed,
            counts.retries_accepted,
            counts.durable
        ),
        (1, 1, 0)
    );
    let now = Instant::now();
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .leaves
        .get_mut(&key(1))
        .unwrap()
        .deadline = now;
    assert!(
        !app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .checkpoint_save_needed(now)
    );
    assert_eq!(
        phase(&app, 1, 1),
        LeavePhase::Failed(LeaveFailure::Deadline)
    );
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .retry_leave(key(1), 1, Instant::now(), MAX_ATTEMPT)
        .unwrap();
    writer.submit(app.world_mut()).unwrap();
    finish(&mut writer, &mut app);
    assert!(matches!(phase(&app, 1, 1), LeavePhase::Durable { .. }));
}

#[test]
fn terminal_worker_loss_fails_submitted_and_waiting_obligations_and_refuses_new_work() {
    let mut app = app();
    request(&mut app, 1, 1);
    let (_directory, mut writer) = writer();
    writer.submit(app.world_mut()).unwrap();
    request(&mut app, 2, 2);
    // Exercise the exact result observer used by a lost worker, including a
    // leave accepted after its current submission. Worker disconnection itself
    // is covered by persistence::worker tests using the channel harness.
    writer.observe(app.world_mut(), &Err(SaveWorkerDisconnected.into()));
    for (account, epoch) in [(1, 1), (2, 2)] {
        assert_eq!(
            phase(&app, account, epoch),
            LeavePhase::Failed(LeaveFailure::Worker)
        );
    }
    assert_eq!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_counts()
            .worker_failed,
        2
    );
    {
        let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
        assert!(
            registry
                .retry_leave(key(1), 1, Instant::now(), MAX_ATTEMPT)
                .is_err()
        );
        attach(&mut registry, 3, 3);
        assert!(
            registry
                .request_leave(key(3), 3, 1, Instant::now(), MAX_ATTEMPT)
                .is_err()
        );
        assert_eq!(registry.activated_epochs(), vec![3]);
    }
    // Drain the real fixture's write before releasing its temporary directory;
    // its physical success cannot undo the injected terminal failure either.
    finish(&mut writer, &mut app);
    assert_eq!(phase(&app, 1, 1), LeavePhase::Failed(LeaveFailure::Worker));
    assert_eq!(
        app.world()
            .resource::<PlayerRegistry>()
            .leave_counts()
            .worker_failed,
        2
    );
}

#[test]
fn invalid_identity_revision_and_unactivated_request_never_quiesce_player() {
    let mut registry = registry();
    attach(&mut registry, 1, 1);
    let checkpoint = registry.projection(1).unwrap().checkpoint;
    for (account, epoch, revision) in [(2, 1, 1), (1, 2, 1), (1, 1, 2)] {
        assert!(
            registry
                .request_leave(key(account), epoch, revision, Instant::now(), MAX_ATTEMPT)
                .is_err()
        );
        assert_eq!(registry.activated_epochs(), vec![1]);
        assert!(registry.leaves.is_empty());
    }
    registry.reserve(key(2), 2, checkpoint).unwrap();
    assert!(
        registry
            .request_leave(key(2), 2, 1, Instant::now(), MAX_ATTEMPT)
            .is_err()
    );
}

#[test]
fn elapsed_operation_deadline_rejects_its_own_successful_write_receipt() {
    let mut app = app();
    request(&mut app, 1, 1);
    let before = app.world().resource::<PlayerRegistry>().snapshot();
    let (_directory, mut writer) = writer();
    writer.submit(app.world_mut()).unwrap();
    // Deterministically cross the operation deadline after submission, without
    // delaying the filesystem or relying on scheduler timing.
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .leaves
        .get_mut(&key(1))
        .unwrap()
        .deadline = Instant::now();
    finish(&mut writer, &mut app);
    assert_eq!(
        phase(&app, 1, 1),
        LeavePhase::Failed(LeaveFailure::Deadline)
    );
    let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
    assert_eq!(registry.snapshot(), before);
    registry.detach(1);
    assert!(registry.settle_leave(key(1), 1).is_err());
    assert!(
        registry
            .reserve(key(1), 3, before.projection(key(1)).unwrap().checkpoint)
            .is_err()
    );
}

#[test]
fn shutdown_teardown_keeps_cutoff_for_final_associated_save_without_destination() {
    let mut app = app();
    request(&mut app, 1, 1);
    let before = app.world().resource::<PlayerRegistry>().snapshot();
    crate::network::teardown(app.world_mut());
    assert!(
        app.world()
            .resource::<PlayerRegistry>()
            .active_epochs()
            .is_empty()
    );
    let (_directory, mut writer) = writer();
    let deadline = Instant::now() + Duration::from_secs(5);
    writer.submit_before(app.world_mut(), deadline).unwrap();
    let receipt = writer
        .finish_before(app.world_mut(), deadline)
        .unwrap()
        .unwrap();
    assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), before);
    assert_eq!(
        phase(&app, 1, 1),
        LeavePhase::Durable {
            generation: receipt.generation,
            tick: receipt.tick
        }
    );
    app.world_mut()
        .resource_mut::<PlayerRegistry>()
        .settle_leave(key(1), 1)
        .unwrap();
    let counts = app.world().resource::<PlayerRegistry>().leave_counts();
    assert_eq!(
        (
            counts.durable,
            counts.detached_settled,
            counts.durable_unsettled
        ),
        (1, 1, 0)
    );
}

#[test]
fn detached_settlement_requires_exact_durability_and_retains_live_or_failed_cutoffs() {
    let mut app = app();
    request(&mut app, 1, 1);
    request(&mut app, 2, 2);
    let (_directory, mut writer) = writer();
    writer.submit(app.world_mut()).unwrap();
    {
        let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
        registry.detach(1);
        assert_eq!(registry.settle_detached_durable_leaves(), 0);
        assert!(registry.leave_status(key(1), 1).is_some());
    }
    finish(&mut writer, &mut app);
    let mut registry = app.world_mut().resource_mut::<PlayerRegistry>();
    let snapshot = registry.snapshot();
    assert_eq!(registry.settle_detached_durable_leaves(), 1);
    assert!(registry.leave_status(key(1), 1).is_none());
    assert!(registry.leave_status(key(2), 2).is_some());
    assert_eq!(registry.settle_detached_durable_leaves(), 0);
    attach(&mut registry, 3, 3);
    registry
        .request_leave(key(3), 3, 1, Instant::now(), MAX_ATTEMPT)
        .unwrap();
    registry.detach(3);
    assert!(!registry.checkpoint_save_needed(Instant::now() + MAX_ATTEMPT));
    assert_eq!(registry.settle_detached_durable_leaves(), 0);
    assert_eq!(
        registry.leave_status(key(3), 3).unwrap().phase,
        LeavePhase::Failed(LeaveFailure::Deadline)
    );
    assert!(
        registry
            .reserve(key(3), 4, snapshot.projection(key(1)).unwrap().checkpoint)
            .is_err()
    );
    registry.detach(2);
    assert_eq!(registry.settle_detached_durable_leaves(), 1);
    assert!(registry.leave_status(key(2), 2).is_none());
    assert!(registry.leave_status(key(3), 3).is_some());
}
