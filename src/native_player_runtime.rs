//! In-memory checkpoint authority, retained independently of network peers.
use crate::native_players::{
    MAX_PLAYERS_PER_ACCOUNT, PlayerCheckpoint, PlayerContext, PlayerError, PlayerKey,
    PlayerProjection, PlayerRecord, PlayerSnapshot,
};
use bevy::prelude::{Resource, World};
use std::collections::BTreeMap;

#[derive(Clone)]
struct Attachment {
    key: PlayerKey,
    pending: Option<PlayerRecord>,
    is_new: bool,
    activated: bool,
}
#[derive(Resource)]
pub struct PlayerRegistry {
    snapshot: PlayerSnapshot,
    attachments: BTreeMap<u64, Attachment>,
    max_live: usize,
    epoch_high_water: u64,
}
impl std::fmt::Debug for PlayerRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PlayerRegistry")
            .field("records", &self.snapshot.len())
            .field("attachments", &self.attachments.len())
            .finish()
    }
}
impl PlayerRegistry {
    pub fn new(
        mut snapshot: PlayerSnapshot,
        expected: &PlayerContext,
        max_live: usize,
    ) -> Result<Self, PlayerError> {
        snapshot.validate()?;
        if snapshot.context() != expected {
            return Err(PlayerError::InvalidContext);
        }
        if !(1..=64).contains(&max_live) {
            return Err(PlayerError::Limit);
        }
        snapshot.records.sort_unstable_by_key(|record| record.key);
        Ok(Self {
            snapshot,
            attachments: BTreeMap::new(),
            max_live,
            epoch_high_water: 0,
        })
    }
    pub fn snapshot(&self) -> PlayerSnapshot {
        self.snapshot.clone()
    }
    pub fn reserve(
        &mut self,
        key: PlayerKey,
        epoch: u64,
        initial: PlayerCheckpoint,
    ) -> Result<PlayerProjection, PlayerError> {
        if epoch == 0 {
            return Err(PlayerError::InvalidIdentity);
        }
        if let Some(held) = self.attachments.get(&epoch) {
            if held.key != key {
                return Err(PlayerError::Conflict);
            }
            return self.projection(epoch).ok_or(PlayerError::Missing);
        }
        if epoch <= self.epoch_high_water {
            return Err(PlayerError::Conflict);
        }
        if self.attachments.len() >= self.max_live {
            return Err(PlayerError::Limit);
        }
        if self.attachments.values().any(|a| a.key == key) {
            return Err(PlayerError::Conflict);
        }
        let pending = if self.snapshot.projection(key).is_some() {
            None
        } else {
            initial.validate()?;
            let pending = self
                .attachments
                .values()
                .filter_map(|a| a.pending.as_ref())
                .collect::<Vec<_>>();
            PlayerSnapshot::check_capacity(
                self.snapshot
                    .len()
                    .checked_add(pending.len())
                    .and_then(|n| n.checked_add(1))
                    .ok_or(PlayerError::Limit)?,
            )?;
            let count = self
                .snapshot
                .records
                .iter()
                .filter(|r| r.key.account == key.account)
                .count()
                + pending
                    .iter()
                    .filter(|r| r.key.account == key.account)
                    .count();
            if count >= MAX_PLAYERS_PER_ACCOUNT {
                return Err(PlayerError::Limit);
            }
            let id = self
                .snapshot
                .high_water
                .checked_add(1)
                .ok_or(PlayerError::Exhausted)?;
            Some(PlayerRecord {
                key,
                id,
                revision: 1,
                checkpoint: initial,
            })
        };
        let is_new = pending.is_some();
        if let Some(r) = &pending {
            self.snapshot.high_water = r.id;
        }
        self.epoch_high_water = epoch;
        self.attachments.insert(
            epoch,
            Attachment {
                key,
                pending,
                is_new,
                activated: false,
            },
        );
        self.projection(epoch).ok_or(PlayerError::Missing)
    }
    pub fn activate(&mut self, epoch: u64) -> Result<PlayerProjection, PlayerError> {
        if self
            .attachments
            .get(&epoch)
            .ok_or(PlayerError::Missing)?
            .pending
            .is_some()
        {
            self.snapshot
                .records
                .try_reserve(1)
                .map_err(|_| PlayerError::Limit)?;
        }
        let held = self
            .attachments
            .get_mut(&epoch)
            .ok_or(PlayerError::Missing)?;
        if let Some(record) = held.pending.take() {
            self.snapshot.records.push(record);
            self.snapshot.records.sort_by_key(|r| r.key);
        }
        held.activated = true;
        self.projection(epoch).ok_or(PlayerError::Missing)
    }
    pub fn cancel(&mut self, epoch: u64) -> bool {
        self.attachments.remove(&epoch).is_some()
    }
    pub fn detach(&mut self, epoch: u64) -> bool {
        self.cancel(epoch)
    }
    pub fn projection(&self, epoch: u64) -> Option<PlayerProjection> {
        let a = self.attachments.get(&epoch)?;
        a.pending
            .as_ref()
            .map(|r| r.projection(a.is_new))
            .or_else(|| {
                self.snapshot
                    .records
                    .iter()
                    .find(|r| r.key == a.key)
                    .map(|r| r.projection(a.is_new))
            })
    }
    /// In-memory acceptance only. A later successful snapshot publishes this revision.
    pub fn edit(
        &mut self,
        key: PlayerKey,
        expected_revision: u64,
        checkpoint: PlayerCheckpoint,
    ) -> Result<PlayerProjection, PlayerError> {
        checkpoint.validate()?;
        let r = self
            .snapshot
            .records
            .iter_mut()
            .find(|r| r.key == key)
            .ok_or(PlayerError::Missing)?;
        let revision = prepare_checkpoint_edit(r, expected_revision, &checkpoint)?;
        r.checkpoint = checkpoint;
        r.revision = revision;
        Ok(r.projection(false))
    }
    /// Activated attachments only: pending returning records do not consume.
    pub fn activated_epochs(&self) -> Vec<u64> {
        self.attachments
            .iter()
            .filter_map(|(epoch, held)| held.activated.then_some(*epoch))
            .collect()
    }

    /// One authored period, in memory only; caller owns cadence and failure latching.
    /// Select at most 64 attached records, never scan cold records for scheduling.
    /// All selected invariants and revision increments pass before any mutation.
    /// Returns changed records; zero currents do not consume revisions. Detached
    /// records and reservations remain unchanged, with no offline catch-up.
    pub fn consume_activated(&mut self, rates: ConsumptionRates) -> Result<usize, PlayerError> {
        if !(1..=64).contains(&self.max_live) || self.attachments.len() > self.max_live {
            return Err(PlayerError::Limit);
        }
        let mut edits = Vec::with_capacity(self.attachments.len());
        let mut selected_ids = std::collections::BTreeSet::new();
        for held in self.attachments.values().filter(|held| held.activated) {
            if held.pending.is_some() {
                return Err(PlayerError::InvalidState);
            }
            let index = self
                .snapshot
                .records
                .binary_search_by_key(&held.key, |record| record.key)
                .map_err(|_| PlayerError::Missing)?;
            let record = &self.snapshot.records[index];
            validate_selected_record(record, self.snapshot.high_water)?;
            if !selected_ids.insert(record.id) {
                return Err(PlayerError::Duplicate);
            }
            let mut next = record.checkpoint;
            next.vitals.oxygen = next.vitals.oxygen.saturating_sub(rates.oxygen).max(0);
            next.vitals.water = next.vitals.water.saturating_sub(rates.water).max(0);
            next.vitals.food = next.vitals.food.saturating_sub(rates.food).max(0);
            if next == record.checkpoint {
                continue;
            }
            let revision = prepare_checkpoint_edit(record, record.revision, &next)?;
            edits.push((index, record.revision, revision, next));
        }
        // Validate the complete prepared CAS set before the first commit.
        for (index, expected, _, _) in &edits {
            if self.snapshot.records[*index].revision != *expected {
                return Err(PlayerError::StaleRevision);
            }
        }
        let changed = edits.len();
        for (index, _, revision, checkpoint) in edits {
            let record = &mut self.snapshot.records[index];
            record.checkpoint = checkpoint;
            record.revision = revision;
        }
        Ok(changed)
    }

    /// All live attachments, including reservations; retained for bridge cleanup.
    pub fn active_epochs(&self) -> Vec<u64> {
        self.attachments.keys().copied().collect()
    }
}
/// Configure an already activated schema-13 registry, never migrate old state.
pub fn install(world: &mut World, expected: &PlayerContext, max_live: usize) -> anyhow::Result<()> {
    let mut registry = world
        .get_resource_mut::<PlayerRegistry>()
        .ok_or(PlayerError::Missing)?;
    if registry.snapshot.context() != expected {
        return Err(PlayerError::InvalidContext.into());
    }
    if !(1..=64).contains(&max_live) || !registry.attachments.is_empty() {
        return Err(PlayerError::Conflict.into());
    }
    registry.max_live = max_live;
    Ok(())
}

/// Native operator-authored units per period; no original consumption rules implied.
#[derive(Clone, Copy, Debug)]
pub struct ConsumptionRates {
    oxygen: i32,
    water: i32,
    food: i32,
}
impl ConsumptionRates {
    pub fn new(oxygen: i32, water: i32, food: i32) -> Result<Self, PlayerError> {
        if [oxygen, water, food]
            .iter()
            .any(|rate| !(0..=10_000_000).contains(rate))
            || (oxygen == 0 && water == 0 && food == 0)
        {
            return Err(PlayerError::InvalidState);
        }
        Ok(Self {
            oxygen,
            water,
            food,
        })
    }
}

fn validate_selected_record(record: &PlayerRecord, high_water: u64) -> Result<(), PlayerError> {
    PlayerKey::new(record.key.account, record.key.slot)?;
    if record.id == 0 || record.id > high_water || record.revision == 0 {
        return Err(PlayerError::InvalidState);
    }
    record.checkpoint.validate()
}
fn prepare_checkpoint_edit(
    record: &PlayerRecord,
    expected_revision: u64,
    checkpoint: &PlayerCheckpoint,
) -> Result<u64, PlayerError> {
    checkpoint.validate()?;
    if record.revision != expected_revision {
        return Err(PlayerError::StaleRevision);
    }
    if !record.checkpoint.same_recipe(checkpoint) {
        return Err(PlayerError::InvalidContext);
    }
    record.revision.checked_add(1).ok_or(PlayerError::Exhausted)
}

#[cfg(test)]
mod consumption_tests {
    use super::*;
    use crate::experimental::{
        pawn_open::CharacterVitals,
        release::{ReleasePin, network_checksum},
    };

    fn initial() -> PlayerCheckpoint {
        PlayerCheckpoint {
            location_cm: [100., 200., 300.],
            vitals: CharacterVitals {
                health: 90,
                max_health: 100,
                stamina: 80,
                max_stamina: 100,
                oxygen: 10,
                max_oxygen: 100,
                water: 20,
                max_water: 100,
                food: 30,
                max_food: 100,
                biome_row: 7,
                growth_row: 9,
            },
        }
    }
    fn registry() -> PlayerRegistry {
        let release = ReleasePin {
            app_id: 2089300,
            build_id: 7,
            project_version: "authored".into(),
            network_checksum: network_checksum("authored"),
        };
        let context =
            PlayerContext::new(vec![65], "/Authored/Map".into(), &release, [1; 32]).unwrap();
        PlayerRegistry::new(PlayerSnapshot::empty(context.clone()), &context, 64).unwrap()
    }
    fn key(account: u64) -> PlayerKey {
        PlayerKey::new(account, 3).unwrap()
    }
    fn rates() -> ConsumptionRates {
        ConsumptionRates::new(2, 3, 4).unwrap()
    }
    fn attach(registry: &mut PlayerRegistry, account: u64, epoch: u64) {
        registry.reserve(key(account), epoch, initial()).unwrap();
        registry.activate(epoch).unwrap();
    }

    #[test]
    fn pending_new_and_returning_attachments_never_consume_before_activation() {
        let mut registry = registry();
        registry.reserve(key(1), 1, initial()).unwrap();
        assert_eq!(registry.active_epochs(), vec![1]);
        assert!(registry.activated_epochs().is_empty());
        assert_eq!(registry.consume_activated(rates()).unwrap(), 0);
        assert_eq!(registry.projection(1).unwrap().checkpoint, initial());
        registry.activate(1).unwrap();
        registry.activate(1).unwrap();
        assert_eq!(registry.activated_epochs(), vec![1]);
        assert_eq!(registry.consume_activated(rates()).unwrap(), 1);
        let saved = registry.snapshot();
        registry.detach(1);
        registry.reserve(key(1), 2, initial()).unwrap();
        assert!(!registry.projection(2).unwrap().is_new);
        assert!(registry.activated_epochs().is_empty());
        assert_eq!(registry.consume_activated(rates()).unwrap(), 0);
        assert_eq!(registry.snapshot(), saved);
        registry.activate(2).unwrap();
        assert_eq!(registry.consume_activated(rates()).unwrap(), 1);
        assert_eq!(registry.projection(2).unwrap().revision, 3);
    }

    #[test]
    fn consumption_changes_only_three_currents_with_one_revision_and_clamps_zero() {
        let mut registry = registry();
        attach(&mut registry, 1, 1);
        assert_eq!(registry.consume_activated(rates()).unwrap(), 1);
        let after = registry.projection(1).unwrap();
        let mut expected = initial();
        expected.vitals.oxygen = 8;
        expected.vitals.water = 17;
        expected.vitals.food = 26;
        assert_eq!(after.checkpoint, expected);
        assert_eq!(after.revision, 2);
        let large = ConsumptionRates::new(10_000_000, 10_000_000, 10_000_000).unwrap();
        assert_eq!(registry.consume_activated(large).unwrap(), 1);
        let zero = registry.snapshot();
        let after = registry.projection(1).unwrap();
        expected.vitals.oxygen = 0;
        expected.vitals.water = 0;
        expected.vitals.food = 0;
        assert_eq!(after.checkpoint, expected);
        assert_eq!(after.revision, 3);
        assert_eq!(registry.consume_activated(large).unwrap(), 0);
        assert_eq!(registry.snapshot(), zero);
    }

    #[test]
    fn detach_and_cancel_exclude_epochs_without_deleting_checkpoint() {
        let mut registry = registry();
        attach(&mut registry, 1, 1);
        attach(&mut registry, 2, 2);
        registry.detach(1);
        registry.cancel(2);
        let before = registry.snapshot();
        assert!(registry.active_epochs().is_empty());
        assert!(registry.activated_epochs().is_empty());
        assert_eq!(registry.consume_activated(rates()).unwrap(), 0);
        assert_eq!(registry.snapshot(), before);
        assert_eq!(before.len(), 2);
        assert!(registry.activate(1).is_err());
        assert!(!registry.detach(1));
        registry.reserve(key(1), 3, initial()).unwrap();
        assert_eq!(registry.consume_activated(rates()).unwrap(), 0);
        registry.activate(3).unwrap();
        assert_eq!(registry.consume_activated(rates()).unwrap(), 1);
        assert_eq!(
            registry.snapshot().projection(key(2)).unwrap().checkpoint,
            initial()
        );
    }

    #[test]
    fn last_record_exhaustion_or_invalid_state_aborts_every_prepared_edit() {
        for failure in [
            "revision",
            "id",
            "duplicate_id",
            "current",
            "missing",
            "pending",
        ] {
            let mut registry = registry();
            attach(&mut registry, 1, 1);
            attach(&mut registry, 2, 2);
            match failure {
                "revision" => registry.snapshot.records[1].revision = u64::MAX,
                "id" => registry.snapshot.records[1].id = 0,
                "duplicate_id" => registry.snapshot.records[1].id = registry.snapshot.records[0].id,
                "current" => registry.snapshot.records[1].checkpoint.vitals.food = 101,
                "missing" => {
                    registry.snapshot.records.pop();
                }
                "pending" => {
                    registry.attachments.get_mut(&2).unwrap().pending =
                        Some(registry.snapshot.records[1].clone())
                }
                _ => unreachable!(),
            }
            let before = registry.snapshot();
            assert!(
                registry.consume_activated(rates()).is_err(),
                "failure={failure}"
            );
            assert_eq!(registry.snapshot(), before, "failure={failure}");
            assert_eq!(registry.snapshot().projection(key(1)).unwrap().revision, 1);
        }
    }

    #[test]
    fn maximum_live_batch_is_bounded_and_uses_canonical_record_lookup() {
        let mut registry = registry();
        // Readiness order may differ from key order; activation canonicalizes records.
        for epoch in 1..=64 {
            attach(&mut registry, 65 - epoch, epoch);
        }
        let before = registry.snapshot();
        assert_eq!(registry.activated_epochs().len(), 64);
        assert_eq!(registry.consume_activated(rates()).unwrap(), 64);
        assert_eq!(registry.snapshot().len(), 64);
        for epoch in registry.activated_epochs() {
            let after = registry.projection(epoch).unwrap();
            assert_eq!(after.revision, 2);
            assert_eq!(after.checkpoint.vitals.oxygen, 8);
        }
        assert!(registry.reserve(key(100), 65, initial()).is_err());
        assert_eq!(before.len(), 64);
    }

    #[test]
    fn retained_input_order_cannot_break_selected_record_binary_search() {
        let mut registry = registry();
        attach(&mut registry, 1, 1);
        attach(&mut registry, 2, 2);
        let mut snapshot = registry.snapshot();
        snapshot.records.reverse();
        let context = snapshot.context().clone();
        let mut restored = PlayerRegistry::new(snapshot, &context, 64).unwrap();
        restored.reserve(key(1), 1, initial()).unwrap();
        restored.activate(1).unwrap();
        assert_eq!(restored.consume_activated(rates()).unwrap(), 1);
        assert_eq!(
            restored
                .snapshot()
                .projection(key(1))
                .unwrap()
                .checkpoint
                .vitals
                .food,
            26
        );
        assert_eq!(
            restored
                .snapshot()
                .projection(key(2))
                .unwrap()
                .checkpoint
                .vitals
                .food,
            30
        );
    }

    #[test]
    fn authored_rate_bounds_refuse_and_zero_rate_leaves_selected_current_unchanged() {
        for values in [(0, 0, 0), (-1, 1, 1), (1, 10_000_001, 1), (1, 1, i32::MAX)] {
            assert!(ConsumptionRates::new(values.0, values.1, values.2).is_err());
        }
        let mut registry = registry();
        attach(&mut registry, 1, 1);
        registry
            .consume_activated(ConsumptionRates::new(0, 3, 0).unwrap())
            .unwrap();
        let current = registry.projection(1).unwrap().checkpoint.vitals;
        assert_eq!((current.oxygen, current.water, current.food), (10, 17, 30));
    }
    #[test]
    fn configured_attachment_limit_aborts_without_touching_live_records() {
        let mut registry = registry();
        attach(&mut registry, 1, 1);
        attach(&mut registry, 2, 2);
        // Model an invalid runtime reconfiguration; normal admission cannot exceed it.
        registry.max_live = 1;
        let before = registry.snapshot();
        assert!(matches!(
            registry.consume_activated(rates()),
            Err(PlayerError::Limit)
        ));
        assert_eq!(registry.snapshot(), before);
    }

    #[test]
    fn exhausted_unchanged_record_does_not_block_other_players_or_advance_revision() {
        let mut registry = registry();
        attach(&mut registry, 1, 1);
        attach(&mut registry, 2, 2);
        let depleted = &mut registry.snapshot.records[0];
        depleted.checkpoint.vitals.oxygen = 0;
        depleted.checkpoint.vitals.water = 0;
        depleted.checkpoint.vitals.food = 0;
        depleted.revision = u64::MAX;
        let before = registry.projection(1).unwrap();
        assert_eq!(registry.consume_activated(rates()).unwrap(), 1);
        assert_eq!(registry.projection(1).unwrap(), before);
        assert_eq!(registry.projection(2).unwrap().revision, 2);
    }

    #[test]
    fn consumed_checkpoint_round_trips_and_only_reactivated_players_resume() {
        let mut registry = registry();
        attach(&mut registry, 1, 1);
        attach(&mut registry, 2, 2);
        registry.detach(2);
        assert_eq!(registry.consume_activated(rates()).unwrap(), 1);
        let before = registry.snapshot();
        let encoded = serde_json::to_vec(&before).unwrap();
        let decoded: PlayerSnapshot = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, before);
        let context = decoded.context().clone();
        let mut restored = PlayerRegistry::new(decoded, &context, 64).unwrap();
        assert_eq!(restored.consume_activated(rates()).unwrap(), 0);
        assert_eq!(restored.snapshot(), before);
        restored.reserve(key(1), 1, initial()).unwrap();
        assert_eq!(restored.consume_activated(rates()).unwrap(), 0);
        restored.activate(1).unwrap();
        assert_eq!(restored.consume_activated(rates()).unwrap(), 1);
        assert_eq!(restored.projection(1).unwrap().revision, 3);
        assert_eq!(restored.projection(1).unwrap().checkpoint.vitals.food, 22);
        assert_eq!(
            restored.snapshot().projection(key(2)).unwrap().checkpoint,
            initial()
        );
    }
}

/// Independently authored integer units per simulated second. No depletion
/// damage, regeneration, temperature, or original gameplay rates are inferred.
#[derive(Clone, Copy, Debug, serde::Serialize, serde::Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ConsumptionConfig {
    pub oxygen: i32,
    pub water: i32,
    pub food: i32,
}
impl ConsumptionConfig {
    pub fn validate(self) -> Result<(), PlayerError> {
        ConsumptionRates::new(self.oxygen, self.water, self.food).map(|_| ())
    }
    fn for_tick(self, tick: u64, tick_hz: u32) -> ConsumptionRates {
        let delta = |rate: i32| {
            let before = u128::from(tick) * rate as u128 / u128::from(tick_hz);
            let after = (u128::from(tick) + 1) * rate as u128 / u128::from(tick_hz);
            (after - before) as i32
        };
        ConsumptionRates {
            oxygen: delta(self.oxygen),
            water: delta(self.water),
            food: delta(self.food),
        }
    }
}

#[derive(Resource)]
struct ConsumptionRuntime {
    failed: bool,
}

/// Failure remains latched until process restart; capture must never publish a
/// state after an unsuccessful authored gameplay step.
pub fn consumption_runtime_health(world: &World) -> anyhow::Result<()> {
    anyhow::ensure!(
        !world
            .get_resource::<ConsumptionRuntime>()
            .is_some_and(|state| state.failed),
        "authored player consumption failed"
    );
    Ok(())
}

pub(crate) fn install_consumption(
    app: &mut bevy::prelude::App,
    config: ConsumptionConfig,
    tick_hz: u32,
) {
    use bevy::prelude::{FixedUpdate, IntoScheduleConfigs};
    app.insert_resource(ConsumptionRuntime { failed: false });
    app.add_systems(
        FixedUpdate,
        (move |world: &mut World| {
            if consumption_runtime_health(world).is_err() {
                return;
            }
            let result = (|| -> Result<(), PlayerError> {
                let tick = world
                    .get_resource::<crate::simulation::SimulationClock>()
                    .ok_or(PlayerError::Missing)?
                    .tick;
                // The phase is the saved simulation tick, so restart neither loses
                // fractions nor charges elapsed wall time. A missing owner fails closed.
                world
                    .get_resource_mut::<PlayerRegistry>()
                    .ok_or(PlayerError::Missing)?
                    .consume_activated(config.for_tick(tick, tick_hz))?;
                Ok(())
            })();
            if result.is_err() {
                world.resource_mut::<ConsumptionRuntime>().failed = true;
                crate::network::stop_admission(world);
            } else {
                crate::network::drain_player_bridge(world);
            }
        })
        .after(crate::simulation::SimulationSet::Gameplay)
        .before(crate::simulation::SimulationSet::Finish),
    );
}

#[cfg(test)]
mod consumption_phase_tests {
    use super::*;
    #[test]
    fn rational_phase_is_exact_for_every_supported_rate_and_extreme_tick() {
        let config = ConsumptionConfig {
            oxygen: 1,
            water: 239,
            food: 10_000_000,
        };
        for hz in 1..=240 {
            for start in [0, 3, u64::MAX - u64::from(hz)] {
                let mut sums = [0_i64; 3];
                for offset in 0..hz {
                    let rates = config.for_tick(start + u64::from(offset), hz);
                    for (sum, rate) in sums.iter_mut().zip([rates.oxygen, rates.water, rates.food])
                    {
                        *sum += i64::from(rate);
                    }
                }
                assert_eq!(sums, [1, 239, 10_000_000]);
            }
        }
    }
    #[test]
    fn missing_registry_latches_failure_and_rejects_capture_health() {
        let mut app = bevy::prelude::App::new();
        app.insert_resource(crate::simulation::SimulationClock { tick: 0 });
        install_consumption(
            &mut app,
            ConsumptionConfig {
                oxygen: 1,
                water: 0,
                food: 0,
            },
            30,
        );
        app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
        assert!(consumption_runtime_health(app.world()).is_err());
        app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
        assert!(consumption_runtime_health(app.world()).is_err());
    }
}
