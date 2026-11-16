//! Authored checkpoint cutoff and single-writer receipt association. This is
//! deliberately not a stock-client leave protocol or a complete player save.
use super::*;
use crate::{
    persistence::{SaveReceipt, SaveWorker, SaveWorkerDisconnected, SnapshotStore},
    simulation::{WorldSnapshot, capture},
};
use anyhow::{Result, ensure};
use std::time::{Duration, Instant};

const MAX_LEAVES: usize = 64;
const MAX_ATTEMPT: Duration = Duration::from_secs(60);

/// Process-local aggregate transitions and current obligations. No player
/// identity, checkpoint contents or per-player revision is exposed or saved.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct PlayerLeaveCounts {
    pub cutoffs_accepted: u64,
    pub retries_accepted: u64,
    pub durable: u64,
    pub capture_failed: u64,
    pub storage_failed: u64,
    pub worker_failed: u64,
    pub deadline_failed: u64,
    pub detached_settled: u64,
    pub pending: usize,
    pub failed: usize,
    pub durable_unsettled: usize,
    pub last_durable: Option<DurableLeaveReceipt>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize)]
pub struct DurableLeaveReceipt {
    pub generation: u64,
    pub tick: u64,
}

impl PlayerLeaveCounts {
    fn failure(&mut self, failure: LeaveFailure) {
        let count = match failure {
            LeaveFailure::Capture => &mut self.capture_failed,
            LeaveFailure::Storage => &mut self.storage_failed,
            LeaveFailure::Worker => &mut self.worker_failed,
            LeaveFailure::Deadline => &mut self.deadline_failed,
        };
        *count = count.saturating_add(1);
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeaveFailure {
    Capture,
    Storage,
    Worker,
    Deadline,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeavePhase {
    Pending,
    /// No preservation claim. The checkpoint and barrier remain owned until
    /// explicit retry succeeds; timeout cannot cancel or roll back a write.
    Failed(LeaveFailure),
    Durable {
        generation: u64,
        tick: u64,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LeaveStatus {
    pub epoch: u64,
    pub revision: u64,
    pub phase: LeavePhase,
}

pub(super) struct LeaveOperation {
    epoch: u64,
    record: PlayerRecord,
    inventory: Option<PlayerInventoryPin>,
    attempt: u64,
    deadline: Instant,
    phase: LeavePhase,
}
#[derive(Clone, PartialEq)]
struct PlayerInventoryPin {
    recipe_id: String,
    baseline: crate::native_inventory::InventoryInitialTotalsV1,
    bag: crate::native_player_inventory::PlayerBagV1,
}
fn inventory_pin(
    inventory: Option<&crate::native_player_inventory::PlayerInventorySnapshotV1>,
    id: u64,
) -> std::result::Result<Option<PlayerInventoryPin>, PlayerError> {
    inventory
        .map(|inventory| {
            crate::native_inventory::player_inventory_charge(inventory, 8)
                .map_err(|_| PlayerError::Limit)?;
            Ok(PlayerInventoryPin {
                recipe_id: inventory.recipe_id.clone(),
                baseline: inventory.baseline.clone(),
                bag: inventory.bag(id).ok_or(PlayerError::Missing)?.clone(),
            })
        })
        .transpose()
}
impl LeaveOperation {
    fn status(&self) -> LeaveStatus {
        LeaveStatus {
            epoch: self.epoch,
            revision: self.record.revision,
            phase: self.phase,
        }
    }
}

impl PlayerRegistry {
    pub fn leave_counts(&self) -> PlayerLeaveCounts {
        let mut counts = self.leave_counts;
        for operation in self.leaves.values() {
            match operation.phase {
                LeavePhase::Pending => counts.pending += 1,
                LeavePhase::Failed(_) => counts.failed += 1,
                LeavePhase::Durable { .. } => counts.durable_unsettled += 1,
            }
        }
        counts
    }
    /// Synchronous exclusive ECS boundary: validate an activated attachment and
    /// freeze its exact current revision before returning. No queued intent can
    /// slip past a zero-fixed-tick frame. Duplicate identity/revision reuses the
    /// same operation, without extending its deadline or taking another cutoff.
    /// Epochs are registry-validated, monotonic attachment identities, not addresses.
    pub fn request_leave(
        &mut self,
        key: PlayerKey,
        epoch: u64,
        expected_revision: u64,
        now: Instant,
        timeout: Duration,
    ) -> Result<LeaveStatus, PlayerError> {
        self.proof_health()?;
        if let Some(op) = self.leaves.get(&key) {
            return if op.epoch == epoch && op.record.revision == expected_revision {
                Ok(op.status())
            } else {
                Err(PlayerError::Conflict)
            };
        }
        let deadline = attempt_deadline(now, timeout)?;
        // Bound unresolved records INCLUDING detached and durable-but-unsettled
        // records. Refusal precedes every mutation of attachment/checkpoint.
        if self.leaves.len() >= MAX_LEAVES {
            return Err(PlayerError::Limit);
        }
        if self.writer_failed {
            return Err(PlayerError::InvalidState);
        }
        let held = self.attachments.get(&epoch).ok_or(PlayerError::Missing)?;
        if held.key != key || !held.activated || held.pending.is_some() {
            return Err(PlayerError::Conflict);
        }
        let record = self
            .snapshot
            .records
            .binary_search_by_key(&key, |r| r.key)
            .ok()
            .map(|index| &self.snapshot.records[index])
            .ok_or(PlayerError::Missing)?;
        validate_selected_record(record, self.snapshot.high_water)?;
        self.snapshot.context().check_rows(&record.checkpoint)?;
        if record.revision != expected_revision {
            return Err(PlayerError::StaleRevision);
        }
        let op = LeaveOperation {
            epoch,
            record: record.clone(),
            inventory: inventory_pin(self.inventory.as_ref(), record.id)?,
            attempt: 1,
            deadline,
            phase: LeavePhase::Pending,
        };
        let status = op.status();
        self.leaves.insert(key, op);
        self.leave_counts.cutoffs_accepted = self.leave_counts.cutoffs_accepted.saturating_add(1);
        self.attachments
            .get_mut(&epoch)
            .expect("validated attachment")
            .activated = false;
        if let Some(proof) = &self.private_leave_proof {
            let op = self.leaves.get(&key).expect("inserted cutoff");
            proof
                .cutoff(epoch, op.attempt, &op.record, self.snapshot.context())
                .map_err(|_| PlayerError::InvalidState)?;
        }
        Ok(status)
    }

    pub fn leave_status(&self, key: PlayerKey, epoch: u64) -> Option<LeaveStatus> {
        self.leaves
            .get(&key)
            .filter(|op| op.epoch == epoch)
            .map(LeaveOperation::status)
    }

    pub fn is_leaving(&self, epoch: u64) -> bool {
        self.leaves.values().any(|op| op.epoch == epoch)
    }

    /// Explicit operator/domain retry; never reactivates or edits the cutoff.
    /// A new attempt cannot be completed by a previous attempt's late receipt.
    /// Worker loss is terminal for this registry/runner; restart from the store.
    pub fn retry_leave(
        &mut self,
        key: PlayerKey,
        epoch: u64,
        now: Instant,
        timeout: Duration,
    ) -> Result<LeaveStatus, PlayerError> {
        self.proof_health()?;
        if let Some(proof) = &self.private_leave_proof {
            proof.fail(crate::leave_runtime_proof::ProofFailure::Order);
            return Err(PlayerError::InvalidState);
        }
        let deadline = attempt_deadline(now, timeout)?;
        if self.writer_failed {
            return Err(PlayerError::InvalidState);
        }
        let op = self.leaves.get_mut(&key).ok_or(PlayerError::Missing)?;
        if op.epoch != epoch || !matches!(op.phase, LeavePhase::Failed(_)) {
            return Err(PlayerError::Conflict);
        }
        let attempt = op.attempt.checked_add(1).ok_or(PlayerError::Exhausted)?;
        op.attempt = attempt;
        op.deadline = deadline;
        op.phase = LeavePhase::Pending;
        self.leave_counts.retries_accepted = self.leave_counts.retries_accepted.saturating_add(1);
        Ok(op.status())
    }

    /// Safe explicit settlement: durability observed AND original attachment
    /// detached. A lost completion destination needs no wire response; an
    /// operator can inspect durability, then settle. Failure never clears a key.
    pub fn settle_leave(&mut self, key: PlayerKey, epoch: u64) -> Result<(), PlayerError> {
        let op = self.leaves.get(&key).ok_or(PlayerError::Missing)?;
        if op.epoch != epoch
            || !matches!(op.phase, LeavePhase::Durable { .. })
            || self.attachments.contains_key(&epoch)
        {
            return Err(PlayerError::Conflict);
        }
        self.leaves.remove(&key);
        self.leave_counts.detached_settled = self.leave_counts.detached_settled.saturating_add(1);
        Ok(())
    }

    /// Release durable cutoffs whose original attachment has gone away. No
    /// response destination remains for these operations; pending and failed
    /// writes continue to protect their player keys.
    pub fn settle_detached_durable_leaves(&mut self) -> usize {
        let before = self.leaves.len();
        self.leaves.retain(|_, op| {
            !matches!(op.phase, LeavePhase::Durable { .. })
                || self.attachments.contains_key(&op.epoch)
        });
        let settled = before - self.leaves.len();
        self.leave_counts.detached_settled = self
            .leave_counts
            .detached_settled
            .saturating_add(settled as u64);
        settled
    }

    fn expire_leaves(&mut self, now: Instant) {
        for op in self.leaves.values_mut() {
            if op.phase == LeavePhase::Pending && now >= op.deadline {
                op.phase = LeavePhase::Failed(LeaveFailure::Deadline);
                self.leave_counts.failure(LeaveFailure::Deadline);
                if let Some(proof) = &self.private_leave_proof {
                    proof.fail(crate::leave_runtime_proof::ProofFailure::Order);
                }
            }
        }
    }

    /// Call once per runner frame even while the writer is busy. Real monotonic
    /// time, independent of simulation ticks or the periodic autosave deadline.
    pub fn checkpoint_save_needed(&mut self, now: Instant) -> bool {
        self.expire_leaves(now);
        self.leaves
            .values()
            .any(|op| op.phase == LeavePhase::Pending)
    }

    fn fail_pending(&mut self, failure: LeaveFailure) {
        self.writer_failed |= failure == LeaveFailure::Worker;
        for op in self.leaves.values_mut() {
            if op.phase == LeavePhase::Pending {
                op.phase = LeavePhase::Failed(failure);
                self.leave_counts.failure(failure);
                if let Some(proof) = &self.private_leave_proof {
                    proof.fail(crate::leave_runtime_proof::ProofFailure::Mismatch);
                }
            }
        }
    }
}

fn attempt_deadline(now: Instant, timeout: Duration) -> Result<Instant, PlayerError> {
    if timeout.is_zero() || timeout > MAX_ATTEMPT {
        return Err(PlayerError::Limit);
    }
    now.checked_add(timeout).ok_or(PlayerError::Exhausted)
}

// Kept with the actual single writer, never matched by generation or tick.
// At most 64 entries; each pins full authored contents as well as ID/revision.
struct IncludedLeave {
    key: PlayerKey,
    epoch: u64,
    attempt: u64,
    record: PlayerRecord,
    inventory: Option<PlayerInventoryPin>,
}
#[derive(Default)]
struct Submission(Vec<IncludedLeave>);
impl Submission {
    fn capture(world: &World, snapshot: &WorldSnapshot) -> Result<Self> {
        snapshot.validate()?;
        let Some(registry) = world.get_resource::<PlayerRegistry>() else {
            return Ok(Self::default());
        };
        let mut included = Vec::with_capacity(registry.leaves.len());
        for (key, op) in &registry.leaves {
            let record = snapshot.native_players.as_ref().and_then(|players| {
                players
                    .records
                    .binary_search_by_key(key, |record| record.key)
                    .ok()
                    .map(|index| &players.records[index])
            });
            ensure!(
                record == Some(&op.record)
                    && inventory_pin(snapshot.player_inventory.as_ref(), op.record.id)?
                        == op.inventory,
                "leave cutoff differs from captured player record"
            );
            if op.phase == LeavePhase::Pending {
                included.push(IncludedLeave {
                    key: *key,
                    epoch: op.epoch,
                    attempt: op.attempt,
                    record: op.record.clone(),
                    inventory: op.inventory.clone(),
                });
            }
        }
        Ok(Self(included))
    }

    fn complete(self, world: &mut World, result: Result<&SaveReceipt, LeaveFailure>, now: Instant) {
        let Some(mut registry) = world.get_resource_mut::<PlayerRegistry>() else {
            return;
        };
        registry.expire_leaves(now);
        for included in self.0 {
            if let Some(op) = registry.leaves.get_mut(&included.key)
                && op.epoch == included.epoch
                && op.attempt == included.attempt
                && op.record == included.record
                && op.inventory == included.inventory
                && op.phase == LeavePhase::Pending
            {
                let phase = match result {
                    Ok(receipt) => LeavePhase::Durable {
                        generation: receipt.generation,
                        tick: receipt.tick,
                    },
                    Err(failure) => LeavePhase::Failed(failure),
                };
                op.phase = phase;
                match phase {
                    LeavePhase::Durable { generation, tick } => {
                        registry.leave_counts.durable =
                            registry.leave_counts.durable.saturating_add(1);
                        registry.leave_counts.last_durable =
                            Some(DurableLeaveReceipt { generation, tick });
                    }
                    LeavePhase::Failed(failure) => registry.leave_counts.failure(failure),
                    LeavePhase::Pending => (),
                }
                if let Some(proof) = &registry.private_leave_proof {
                    match result {
                        Ok(receipt) => {
                            let _ = proof.durable(included.epoch, included.attempt, receipt);
                        }
                        Err(_) => {
                            proof.fail(crate::leave_runtime_proof::ProofFailure::Mismatch);
                        }
                    }
                }
            }
        }
    }
}

/// Narrow wrapper around the existing writer: every accepted submission owns
/// its exact leave set until its own result is observed. No second writer,
/// schema, event stream or inferred acknowledgment. All runner paths, including
/// shutdown, use this wrapper so an old/equal-tick autosave cannot finish a leave.
pub struct CheckpointWriter {
    worker: SaveWorker,
    submission: Option<Submission>,
}
impl CheckpointWriter {
    pub fn spawn(store: SnapshotStore) -> Result<Self> {
        Ok(Self {
            worker: SaveWorker::spawn(store)?,
            submission: None,
        })
    }
    pub fn is_busy(&self) -> bool {
        self.worker.is_busy()
    }

    pub fn submit(&mut self, world: &mut World) -> Result<()> {
        self.submit_inner(world, None)
    }
    pub fn submit_before(&mut self, world: &mut World, deadline: Instant) -> Result<()> {
        self.submit_inner(world, Some(deadline))
    }
    fn submit_inner(&mut self, world: &mut World, deadline: Option<Instant>) -> Result<()> {
        // Queue pressure refuses before capture or changing any operation.
        ensure!(!self.worker.is_busy(), "snapshot worker busy");
        if let Some(mut registry) = world.get_resource_mut::<PlayerRegistry>() {
            registry.expire_leaves(Instant::now());
        }
        let prepared = capture(world)
            .and_then(|snapshot| Ok((Submission::capture(world, &snapshot)?, snapshot)));
        let (submission, snapshot) = match prepared {
            Ok(prepared) => prepared,
            Err(error) => {
                if let Some(mut registry) = world.get_resource_mut::<PlayerRegistry>() {
                    registry.fail_pending(LeaveFailure::Capture);
                }
                return Err(error);
            }
        };
        let proof_selection = world.get_resource::<PlayerRegistry>().and_then(|registry| {
            let proof = registry.private_leave_proof.as_ref()?;
            if proof.health().is_err() || submission.0.is_empty() {
                return None;
            }
            if submission.0.len() != 1 {
                proof.fail(crate::leave_runtime_proof::ProofFailure::Mismatch);
                return None;
            }
            let included = &submission.0[0];
            snapshot.native_players.as_ref().and_then(|players| {
                proof
                    .writer_selection(included.epoch, included.attempt, &included.record, players)
                    .ok()
            })
        });
        let result = if deadline.is_some_and(|deadline| Instant::now() >= deadline) {
            Err(anyhow::anyhow!("snapshot shutdown deadline exceeded"))
        } else {
            self.worker.submit_private(snapshot, proof_selection)
        };
        if let Err(error) = result {
            let failure = if error.is::<SaveWorkerDisconnected>() {
                LeaveFailure::Worker
            } else {
                LeaveFailure::Deadline
            };
            submission.complete(world, Err(failure), Instant::now());
            if failure == LeaveFailure::Worker {
                Self::worker_lost(world);
            }
            return Err(error);
        }
        self.submission = Some(submission);
        Ok(())
    }

    pub fn poll(&mut self, world: &mut World) -> Result<Option<SaveReceipt>> {
        let result = self.worker.poll();
        self.observe(world, &result);
        result
    }
    fn observe(&mut self, world: &mut World, result: &Result<Option<SaveReceipt>>) {
        if let Some(mut registry) = world.get_resource_mut::<PlayerRegistry>() {
            registry.expire_leaves(Instant::now());
        }
        match result {
            Ok(None) => (),
            Ok(Some(receipt)) => {
                if let Some(submission) = self.submission.take() {
                    submission.complete(world, Ok(receipt), Instant::now());
                }
            }
            Err(error) => {
                let failure = if error.is::<SaveWorkerDisconnected>() {
                    LeaveFailure::Worker
                } else {
                    LeaveFailure::Storage
                };
                if let Some(submission) = self.submission.take() {
                    submission.complete(world, Err(failure), Instant::now());
                }
                if failure == LeaveFailure::Worker {
                    Self::worker_lost(world);
                }
            }
        }
    }
    fn worker_lost(world: &mut World) {
        if let Some(mut registry) = world.get_resource_mut::<PlayerRegistry>() {
            registry.fail_pending(LeaveFailure::Worker);
        }
    }

    pub fn settle_before(
        &mut self,
        world: &mut World,
        deadline: Instant,
    ) -> Result<Option<Result<SaveReceipt>>> {
        match self.worker.settle_before(deadline) {
            Ok(Some(result)) => {
                // Keep the save's own failure separate so final shutdown capture
                // can still run, just as for SaveWorker::settle_before.
                let observed = result.map(Some);
                self.observe(world, &observed);
                Ok(Some(observed.map(|receipt| receipt.expect("completion"))))
            }
            Ok(None) => Ok(None),
            Err(error) => {
                let failure = if error.is::<SaveWorkerDisconnected>() {
                    LeaveFailure::Worker
                } else {
                    LeaveFailure::Deadline
                };
                if let Some(submission) = self.submission.take() {
                    submission.complete(world, Err(failure), Instant::now());
                }
                if failure == LeaveFailure::Worker {
                    Self::worker_lost(world);
                }
                Err(error)
            }
        }
    }
    pub fn finish_before(
        &mut self,
        world: &mut World,
        deadline: Instant,
    ) -> Result<Option<SaveReceipt>> {
        self.settle_before(world, deadline)?.transpose()
    }
}

#[cfg(test)]
mod tests;
