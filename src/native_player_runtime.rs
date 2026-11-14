//! In-memory checkpoint authority, retained independently of network peers.
use crate::native_players::{
    MAX_PLAYERS_PER_ACCOUNT, PlayerCheckpoint, PlayerContext, PlayerError, PlayerKey,
    PlayerProjection, PlayerRecord, PlayerSnapshot,
};
use bevy::prelude::{Res, Resource, World};
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
            self.snapshot.context().check_rows(&initial)?;
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
            self.snapshot.context().check_rows(&record.checkpoint)?;
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
/// Configure an already opened player registry, never migrate old state. A
/// stored snapshot must have been opened against `expected` first
/// ([`PlayerSnapshot::open`]), which resolves its rows to the installed order.
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
                biome_row: 0,
                growth_row: 0,
            },
        }
    }
    pub(super) fn registry() -> PlayerRegistry {
        let release = ReleasePin {
            app_id: 2089300,
            build_id: 7,
            project_version: "authored".into(),
            network_checksum: network_checksum("authored"),
        };
        let context = PlayerContext::new(
            vec![65],
            "/Authored/Map".into(),
            crate::native_players::PlayerInputs {
                authored_gameplay: [1; 32],
                spawn: [1; 32],
                authored_world: [1; 32],
                declaration: [1; 32],
                initial_values: [1; 32],
            },
            vec!["Player".into()],
        )
        .unwrap();
        PlayerRegistry::new(
            PlayerSnapshot::fresh(context.clone(), &release).unwrap(),
            &context,
            64,
        )
        .unwrap()
    }
    fn key(account: u64) -> PlayerKey {
        PlayerKey::new(account, 3).unwrap()
    }
    fn rates() -> ConsumptionRates {
        ConsumptionRates::new(2, 3, 4).unwrap()
    }
    pub(super) fn attach(registry: &mut PlayerRegistry, account: u64, epoch: u64) {
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
            "growth",
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
                "growth" => registry.snapshot.records[1].checkpoint.vitals.growth_row = 1,
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
    fn schema14_day_and_fractional_consumption_resume_from_committed_tick() {
        use crate::{
            authored_world_bootstrap::AuthoredClockConfig,
            config::ServerConfig,
            persistence::SnapshotStore,
            simulation::{
                SNAPSHOT_SCHEMA_PLAYERS, SimulationClock, WorldSnapshot, build_app, capture,
            },
        };
        let registry = registry();
        let snapshot = WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_PLAYERS,
            native_players: Some(registry.snapshot()),
            ..Default::default()
        };
        let config = ServerConfig {
            tick_hz: 7,
            ..Default::default()
        };
        let rates = ConsumptionConfig {
            oxygen: 2,
            water: 3,
            food: 4,
        };
        let clock = AuthoredClockConfig {
            initial_minute: 480,
            seconds_per_day: 2,
            tick_hz: 7,
        }
        .prepare()
        .unwrap();
        let mut app = build_app(&config, &snapshot).unwrap();
        install_consumption(&mut app, rates, 7).unwrap();
        attach(&mut app.world_mut().resource_mut::<PlayerRegistry>(), 1, 1);
        for _ in 0..3 {
            app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
        }
        let partial = capture(app.world_mut()).unwrap();
        assert_eq!(partial.schema_version, 14);
        assert_eq!(partial.tick, 3);
        let directory = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(directory.path()).unwrap();
        store.save(&partial).unwrap();
        let loaded = store.load().unwrap().unwrap().snapshot;
        assert_eq!(loaded.tick, 3);
        assert_eq!(
            clock.sample(loaded.tick).to_bits(),
            clock.sample(partial.tick).to_bits()
        );
        let mut reopened = build_app(&config, &loaded).unwrap();
        install_consumption(&mut reopened, rates, 7).unwrap();
        // Reopening never fabricates live attachments or charges elapsed wall time.
        assert_eq!(
            reopened.world().resource::<PlayerRegistry>().snapshot(),
            *loaded.native_players.as_ref().unwrap()
        );
        attach(
            &mut reopened.world_mut().resource_mut::<PlayerRegistry>(),
            1,
            1,
        );
        for _ in 0..4 {
            reopened
                .world_mut()
                .run_schedule(bevy::prelude::FixedUpdate);
        }
        assert!(consumption_runtime_health(reopened.world()).is_ok());
        assert_eq!(reopened.world().resource::<SimulationClock>().tick, 7);
        assert_eq!(clock.sample(7), 1200.0);
        let projected = reopened
            .world()
            .resource::<PlayerRegistry>()
            .projection(1)
            .unwrap();
        assert_eq!(
            (
                projected.checkpoint.vitals.oxygen,
                projected.checkpoint.vitals.water,
                projected.checkpoint.vitals.food
            ),
            (8, 17, 26)
        );
        let before = reopened.world().resource::<PlayerRegistry>().snapshot();
        reopened
            .world_mut()
            .resource_mut::<PlayerRegistry>()
            .detach(1);
        for _ in 0..7 {
            reopened
                .world_mut()
                .run_schedule(bevy::prelude::FixedUpdate);
        }
        assert_eq!(reopened.world().resource::<SimulationClock>().tick, 14);
        assert_eq!(clock.sample(14), 480.0);
        assert_eq!(
            reopened.world().resource::<PlayerRegistry>().snapshot(),
            before
        );
    }

    #[test]
    fn offline_clock_only_owner_failure_latches_without_tick_or_capture_and_no_feature_is_ordinary()
    {
        use crate::{
            authored_world_bootstrap::{AuthoredClockConfig, AuthoredWorldBootstrap},
            config::{NetworkConfig, PlayerPersistenceConfig, ServerConfig},
            simulation::{
                SNAPSHOT_SCHEMA_PLAYERS, SimulationClock, WorldSnapshot, build_app, capture,
            },
        };
        let config = ServerConfig {
            network: NetworkConfig {
                release_profile: Some("authored-release.toml".into()),
                data_pak: Some("authored-catalog.pak".into()),
                authored_gameplay: Some("authored-gameplay.json".into()),
                player_persistence: Some(PlayerPersistenceConfig {
                    prospect_id: "authored".into(),
                    map_url: "/Authored/Map".into(),
                    prospect_row: "AuthoredRow".into(),
                    difficulty: 2,
                }),
                authored_world: Some(AuthoredWorldBootstrap {
                    seed: 42,
                    seed_initialised: true,
                    flod_tile_count: 0,
                    clock: Some(AuthoredClockConfig {
                        initial_minute: 480,
                        seconds_per_day: 3600,
                        tick_hz: 30,
                    }),
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        for failure in ["clock", "fixed", "timestep"] {
            let players = registry().snapshot();
            let snapshot = WorldSnapshot {
                schema_version: SNAPSHOT_SCHEMA_PLAYERS,
                native_players: Some(players.clone()),
                ..Default::default()
            };
            let mut app = build_app(&config, &snapshot).unwrap();
            assert!(config.network.player_consumption.is_none());
            assert!(
                !app.world()
                    .contains_resource::<crate::experimental::peer_lab::PeerLab>()
            );
            let clock = if failure == "clock" {
                app.world_mut().remove_resource::<SimulationClock>()
            } else {
                None
            };
            let fixed = if failure == "fixed" {
                app.world_mut()
                    .remove_resource::<bevy::prelude::Time<bevy::prelude::Fixed>>()
            } else {
                None
            };
            if failure == "timestep" {
                app.world_mut()
                    .resource_mut::<bevy::prelude::Time<bevy::prelude::Fixed>>()
                    .set_timestep(std::time::Duration::from_secs(1));
            }
            app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
            assert!(crate::network::authored_clock_runtime_health(app.world()).is_err());
            if let Some(clock) = clock {
                app.insert_resource(clock);
            }
            if let Some(fixed) = fixed {
                app.insert_resource(fixed);
            }
            app.world_mut()
                .resource_mut::<bevy::prelude::Time<bevy::prelude::Fixed>>()
                .set_timestep(std::time::Duration::from_secs_f64(1.0 / 30.0));
            app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
            assert!(crate::network::authored_clock_runtime_health(app.world()).is_err());
            assert_eq!(app.world().resource::<SimulationClock>().tick, 0);
            assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), players);
            assert!(capture(app.world_mut()).is_err());
        }
        let mut ordinary = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
        for _ in 0..2 {
            ordinary
                .world_mut()
                .run_schedule(bevy::prelude::FixedUpdate);
        }
        assert_eq!(ordinary.world().resource::<SimulationClock>().tick, 2);
        assert!(capture(ordinary.world_mut()).is_ok());
        assert!(crate::network::authored_clock_runtime_health(ordinary.world()).is_ok());
        assert!(!ordinary.world().contains_resource::<ConsumptionRuntime>());
    }

    #[test]
    fn offline_clockless_installation_rejects_and_latches_wrong_or_missing_fixed_time() {
        use crate::{
            config::ServerConfig,
            simulation::{SNAPSHOT_SCHEMA_PLAYERS, WorldSnapshot, build_app, capture},
        };
        for missing in [false, true] {
            let registry = registry();
            let before = registry.snapshot();
            let snapshot = WorldSnapshot {
                schema_version: SNAPSHOT_SCHEMA_PLAYERS,
                native_players: Some(before.clone()),
                ..Default::default()
            };
            let mut app = build_app(&ServerConfig::default(), &snapshot).unwrap();
            assert!(
                !app.world()
                    .contains_resource::<crate::experimental::peer_lab::PeerLab>()
            );
            let time = if missing {
                app.world_mut()
                    .remove_resource::<bevy::prelude::Time<bevy::prelude::Fixed>>()
            } else {
                app.world_mut()
                    .resource_mut::<bevy::prelude::Time<bevy::prelude::Fixed>>()
                    .set_timestep(std::time::Duration::from_secs(1));
                None
            };
            let rates = ConsumptionConfig {
                oxygen: 0,
                water: 0,
                food: 1,
            };
            assert!(install_consumption(&mut app, rates, 30).is_err());
            assert!(consumption_runtime_health(app.world()).is_err());
            if let Some(time) = time {
                app.insert_resource(time);
            }
            app.world_mut()
                .resource_mut::<bevy::prelude::Time<bevy::prelude::Fixed>>()
                .set_timestep(std::time::Duration::from_secs_f64(1.0 / 30.0));
            assert!(install_consumption(&mut app, rates, 30).is_err());
            assert!(capture(app.world_mut()).is_err());
            assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), before);
        }
    }

    #[test]
    fn failed_owner_or_timestep_latches_and_reinsertion_cannot_publish_capture() {
        use crate::{
            config::ServerConfig,
            simulation::{SNAPSHOT_SCHEMA_PLAYERS, WorldSnapshot, build_app, capture},
        };
        for failure in ["owner", "timestep", "missing-fixed"] {
            let registry = registry();
            let snapshot = WorldSnapshot {
                schema_version: SNAPSHOT_SCHEMA_PLAYERS,
                native_players: Some(registry.snapshot()),
                ..Default::default()
            };
            let mut app = build_app(&ServerConfig::default(), &snapshot).unwrap();
            install_consumption(
                &mut app,
                ConsumptionConfig {
                    oxygen: 0,
                    water: 0,
                    food: 30,
                },
                30,
            )
            .unwrap();
            attach(&mut app.world_mut().resource_mut::<PlayerRegistry>(), 1, 1);
            let before = app.world().resource::<PlayerRegistry>().snapshot();
            let tick_before = app
                .world()
                .resource::<crate::simulation::SimulationClock>()
                .tick;
            let removed = if failure == "owner" {
                app.world_mut().remove_resource::<PlayerRegistry>()
            } else {
                None
            };
            let fixed = if failure == "missing-fixed" {
                app.world_mut()
                    .remove_resource::<bevy::prelude::Time<bevy::prelude::Fixed>>()
            } else {
                None
            };
            if failure == "timestep" {
                app.world_mut()
                    .resource_mut::<bevy::prelude::Time<bevy::prelude::Fixed>>()
                    .set_timestep(std::time::Duration::from_secs(1));
            }
            app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
            assert!(consumption_runtime_health(app.world()).is_err());
            assert_eq!(
                app.world()
                    .resource::<crate::simulation::SimulationClock>()
                    .tick,
                tick_before
            );
            if let Some(registry) = removed {
                app.insert_resource(registry);
            }
            if let Some(fixed) = fixed {
                app.insert_resource(fixed);
            }
            app.world_mut()
                .resource_mut::<bevy::prelude::Time<bevy::prelude::Fixed>>()
                .set_timestep(std::time::Duration::from_secs_f64(1.0 / 30.0));
            app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
            assert!(consumption_runtime_health(app.world()).is_err());
            assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), before);
            assert_eq!(
                app.world()
                    .resource::<crate::simulation::SimulationClock>()
                    .tick,
                tick_before
            );
            assert!(capture(app.world_mut()).is_err());
        }
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

/// Reuse schema14's existing declaration fingerprint. With no enabled rules,
/// retain the exact legacy JSON bytes; all enabled rules bind the actual rate.
pub(crate) fn player_declaration_digest(
    declaration: &crate::config::PlayerPersistenceConfig,
    consumption: Option<ConsumptionConfig>,
    actual_tick_hz: u32,
) -> anyhow::Result<[u8; 32]> {
    use sha2::Digest;
    let bytes = match consumption {
        None => serde_json::to_vec(declaration)?,
        Some(rates) => {
            rates.validate()?;
            anyhow::ensure!(
                (1..=240).contains(&actual_tick_hz),
                "invalid consumption simulation rate"
            );
            serde_json::to_vec(&(
                "authored_player_rules_v1",
                declaration,
                rates,
                actual_tick_hz,
            ))?
        }
    };
    Ok(sha2::Sha256::digest(bytes).into())
}

#[derive(Resource)]
pub(crate) struct ConsumptionRuntime {
    failed: bool,
    tick_hz: u32,
}

/// Failed gameplay intervals cannot advance the durable tick. In particular,
/// removing Time<Fixed> must latch before Finish's required time resource runs.
/// With consumption omitted, this condition preserves the ordinary schedule.
pub(crate) fn consumption_can_finish(state: Option<Res<ConsumptionRuntime>>) -> bool {
    state.is_none_or(|state| !state.failed)
}

fn validate_consumption_timestep(world: &World, tick_hz: u32) -> Result<(), PlayerError> {
    let expected = std::time::Duration::from_secs_f64(1.0 / f64::from(tick_hz));
    if world
        .get_resource::<bevy::prelude::Time<bevy::prelude::Fixed>>()
        .is_none_or(|time| time.timestep() != expected)
    {
        return Err(PlayerError::InvalidContext);
    }
    Ok(())
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

/// Check ownership before Bevy itself reads Fixed time, even on a frame with
/// no fixed interval. This cut validates health only, never authentication or
/// player values; the authoritative eligibility cut remains immediately before debit.
pub(crate) fn reconcile_consumption_runtime(world: &mut World) -> anyhow::Result<()> {
    let Some(runtime) = world.get_resource::<ConsumptionRuntime>() else {
        return Ok(());
    };
    let tick_hz = runtime.tick_hz;
    let result = (|| -> anyhow::Result<()> {
        consumption_runtime_health(world)?;
        anyhow::ensure!(
            world.contains_resource::<crate::simulation::SimulationClock>(),
            "player consumption requires SimulationClock"
        );
        anyhow::ensure!(
            world.contains_resource::<PlayerRegistry>(),
            "player consumption registry missing"
        );
        validate_consumption_timestep(world, tick_hz)?;
        Ok(())
    })();
    if result.is_err() {
        world.resource_mut::<ConsumptionRuntime>().failed = true;
        crate::network::stop_admission(world);
    }
    result
}

pub(crate) fn install_consumption(
    app: &mut bevy::prelude::App,
    config: ConsumptionConfig,
    tick_hz: u32,
) -> anyhow::Result<()> {
    use bevy::prelude::{FixedUpdate, IntoScheduleConfigs};
    config.validate()?;
    anyhow::ensure!(
        (1..=240).contains(&tick_hz),
        "invalid consumption simulation rate"
    );
    anyhow::ensure!(
        !app.world().contains_resource::<ConsumptionRuntime>(),
        "player consumption already installed"
    );
    app.insert_resource(ConsumptionRuntime {
        failed: false,
        tick_hz,
    });
    crate::simulation::install_authored_preloop_guard(app);
    if let Err(error) = validate_consumption_timestep(app.world(), tick_hz) {
        app.world_mut().resource_mut::<ConsumptionRuntime>().failed = true;
        crate::network::stop_admission(app.world_mut());
        return Err(error.into());
    }
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
                if tick == u64::MAX {
                    return Err(PlayerError::Exhausted);
                }
                validate_consumption_timestep(world, tick_hz)?;
                crate::network::reconcile_authored_clock(world)
                    .map_err(|_| PlayerError::InvalidContext)?;
                // This eligibility cut drains already queued auth outcomes and
                // detaches stale epochs only; it cannot publish projections.
                // Later outcomes affect the next interval's cut.
                crate::network::reconcile_player_authority(world)
                    .map_err(|_| PlayerError::InvalidIdentity)?;
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
            }
        })
        .after(crate::simulation::SimulationSet::Gameplay)
        .before(crate::simulation::SimulationSet::Finish),
    );
    app.add_systems(
        FixedUpdate,
        (|world: &mut World| {
            if consumption_runtime_health(world).is_ok()
                && crate::network::reconcile_authored_clock(world).is_ok()
            {
                crate::network::drain_player_bridge(world);
            }
        })
        .after(crate::simulation::SimulationSet::Finish),
    );
    Ok(())
}

#[cfg(test)]
mod consumption_phase_tests {
    use super::*;
    fn declaration() -> crate::config::PlayerPersistenceConfig {
        crate::config::PlayerPersistenceConfig {
            prospect_id: "authored".into(),
            map_url: "/Authored/Map".into(),
            prospect_row: "AuthoredRow".into(),
            difficulty: 2,
        }
    }

    #[test]
    fn omitted_rules_keep_exact_legacy_declaration_bytes_and_digest() {
        let declaration = declaration();
        let expected = br#"{"prospect_id":"authored","map_url":"/Authored/Map","prospect_row":"AuthoredRow","difficulty":2}"#;
        assert_eq!(serde_json::to_vec(&declaration).unwrap(), expected);
        let digest = [
            7, 17, 116, 110, 189, 117, 195, 83, 107, 237, 164, 65, 109, 200, 124, 34, 112, 176, 22,
            162, 61, 236, 204, 144, 243, 20, 7, 215, 75, 242, 77, 81,
        ];
        for hz in [1, 7, 30, 240] {
            assert_eq!(
                player_declaration_digest(&declaration, None, hz).unwrap(),
                digest
            );
        }
        let legacy = serde_json::to_vec(&crate::config::NetworkConfig::default()).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&legacy).unwrap();
        assert!(value.get("player_consumption").is_none());
    }

    #[test]
    fn enabled_rules_bind_declaration_every_rate_and_actual_hz_even_zero_debits() {
        let declaration = declaration();
        let rates = ConsumptionConfig {
            oxygen: 0,
            water: 0,
            food: 1,
        };
        assert_eq!(rates.for_tick(0, 7).food, 0);
        let enabled = player_declaration_digest(&declaration, Some(rates), 7).unwrap();
        assert_ne!(
            enabled,
            player_declaration_digest(&declaration, None, 7).unwrap()
        );
        assert_ne!(
            enabled,
            player_declaration_digest(&declaration, Some(rates), 30).unwrap()
        );
        for changed in [
            ConsumptionConfig { oxygen: 1, ..rates },
            ConsumptionConfig { water: 1, ..rates },
            ConsumptionConfig { food: 2, ..rates },
        ] {
            assert_ne!(
                enabled,
                player_declaration_digest(&declaration, Some(changed), 7).unwrap()
            );
        }
        let mut changed = declaration.clone();
        changed.difficulty = 3;
        assert_ne!(
            enabled,
            player_declaration_digest(&changed, Some(rates), 7).unwrap()
        );
        for hz in [0, 241] {
            assert!(player_declaration_digest(&declaration, Some(rates), hz).is_err());
        }
        let zero = ConsumptionConfig {
            oxygen: 0,
            water: 0,
            food: 0,
        };
        assert!(player_declaration_digest(&declaration, Some(zero), 7).is_err());
    }

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
    fn exhausted_tick_latches_before_mutation_and_cannot_repeat_last_charge() {
        use crate::{
            config::ServerConfig,
            simulation::{SNAPSHOT_SCHEMA_PLAYERS, SimulationClock, WorldSnapshot, build_app},
        };
        for start in [u64::MAX - 1, u64::MAX] {
            let mut registry = super::consumption_tests::registry();
            super::consumption_tests::attach(&mut registry, 1, 1);
            let initial = registry.snapshot();
            let snapshot = WorldSnapshot {
                schema_version: SNAPSHOT_SCHEMA_PLAYERS,
                tick: start,
                native_players: Some(initial.clone()),
                ..Default::default()
            };
            let mut app = build_app(
                &ServerConfig {
                    tick_hz: 1,
                    ..Default::default()
                },
                &snapshot,
            )
            .unwrap();
            app.insert_resource(registry);
            install_consumption(
                &mut app,
                ConsumptionConfig {
                    oxygen: 1,
                    water: 1,
                    food: 1,
                },
                1,
            )
            .unwrap();
            if start < u64::MAX {
                app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
                assert!(consumption_runtime_health(app.world()).is_ok());
                assert_eq!(app.world().resource::<SimulationClock>().tick, u64::MAX);
                assert_eq!(
                    app.world()
                        .resource::<PlayerRegistry>()
                        .projection(1)
                        .unwrap()
                        .revision,
                    2
                );
            }
            let before = app.world().resource::<PlayerRegistry>().snapshot();
            for _ in 0..2 {
                app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
                assert!(consumption_runtime_health(app.world()).is_err());
                assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), before);
            }
        }
    }

    #[test]
    fn missing_registry_latches_failure_and_rejects_capture_health() {
        let mut app = bevy::prelude::App::new();
        app.insert_resource(crate::simulation::SimulationClock { tick: 0 });
        app.insert_resource(bevy::prelude::Time::<bevy::prelude::Fixed>::from_hz(30.0));
        install_consumption(
            &mut app,
            ConsumptionConfig {
                oxygen: 1,
                water: 0,
                food: 0,
            },
            30,
        )
        .unwrap();
        app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
        assert!(consumption_runtime_health(app.world()).is_err());
        assert!(
            install_consumption(
                &mut app,
                ConsumptionConfig {
                    oxygen: 1,
                    water: 0,
                    food: 0
                },
                30
            )
            .is_err()
        );
        app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
        assert!(consumption_runtime_health(app.world()).is_err());
    }
}

#[cfg(test)]
mod authored_preloop_tests {
    use super::*;
    use crate::{
        authored_world_bootstrap::{AuthoredClockConfig, AuthoredWorldBootstrap},
        config::{NetworkConfig, PlayerPersistenceConfig, ServerConfig},
        experimental::{
            peer_lab::{PeerLab, PeerLabConfig, PeerLabPlugin},
            release::ReleaseProfile,
        },
        simulation::{SNAPSHOT_SCHEMA_PLAYERS, SimulationClock, WorldSnapshot, build_app, capture},
    };
    use bevy::{
        prelude::{App, Fixed, Time},
        time::TimeUpdateStrategy,
    };

    fn clock() -> AuthoredClockConfig {
        AuthoredClockConfig {
            initial_minute: 480,
            seconds_per_day: 3600,
            tick_hz: 30,
        }
    }
    fn app(clock_enabled: bool, consumption_enabled: bool) -> App {
        let mut registry = super::consumption_tests::registry();
        super::consumption_tests::attach(&mut registry, 1, 1);
        let snapshot = WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_PLAYERS,
            native_players: Some(registry.snapshot()),
            ..Default::default()
        };
        let config = ServerConfig {
            network: NetworkConfig {
                release_profile: Some("authored-release.toml".into()),
                data_pak: Some("authored-catalog.pak".into()),
                authored_gameplay: Some("authored-gameplay.json".into()),
                player_persistence: Some(PlayerPersistenceConfig {
                    prospect_id: "authored".into(),
                    map_url: "/Authored/Map".into(),
                    prospect_row: "AuthoredRow".into(),
                    difficulty: 2,
                }),
                authored_world: Some(AuthoredWorldBootstrap {
                    seed: 42,
                    seed_initialised: true,
                    flod_tile_count: 0,
                    clock: clock_enabled.then(clock),
                }),
                player_consumption: consumption_enabled.then_some(ConsumptionConfig {
                    oxygen: 30,
                    water: 30,
                    food: 30,
                }),
                ..Default::default()
            },
            ..Default::default()
        };
        let mut app = build_app(&config, &snapshot).unwrap();
        app.insert_resource(registry)
            .insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
        // Warm TimePlugin's initial zero delta, then exercise an ordinary frame.
        app.update();
        app.update();
        assert_eq!(app.world().resource::<SimulationClock>().tick, 1);
        app
    }

    /// A transport-free planned lab: the real poll plugin must return on
    /// failed health before trying to receive or publish. No socket is bound.
    fn add_unbound_lab(app: &mut App, clock_enabled: bool) {
        let config = NetworkConfig {
            enabled: true,
            port: 0,
            release_profile: Some(ReleaseProfile::fixture_path("3.0.35")),
            ..Default::default()
        };
        let mut prepared =
            crate::network::prepare_session(&config, crate::network::peer_lab_config(&config))
                .unwrap()
                .unwrap();
        let mut lab = std::mem::replace(
            &mut prepared.session_mut().lab,
            PeerLab::prepare(PeerLabConfig::default()).unwrap(),
        );
        if clock_enabled {
            lab.enable_authored_time_of_day(clock().prepare().unwrap(), 0)
                .unwrap();
        }
        app.insert_resource(lab).add_plugins(PeerLabPlugin);
    }

    #[test]
    fn normal_app_update_latches_missing_owners_and_wrong_rate_before_drivers_and_poll() {
        // Each feature independently protects normal App::update. Both together
        // additionally prove that installing the shared guard twice is harmless.
        for (clock_enabled, consumption_enabled) in [(true, false), (false, true), (true, true)] {
            for with_lab in [false, true] {
                for failure in ["fixed", "clock", "slow-fixed"] {
                    let mut app = app(clock_enabled, consumption_enabled);
                    let before = app.world().resource::<PlayerRegistry>().snapshot();
                    let tick = app.world().resource::<SimulationClock>().tick;
                    if with_lab {
                        add_unbound_lab(&mut app, clock_enabled);
                    }
                    let sent = app
                        .world()
                        .get_resource::<PeerLab>()
                        .map(|lab| lab.stats().game_state_updates_sent);
                    let time = app
                        .world()
                        .resource::<Time<bevy::prelude::Virtual>>()
                        .elapsed();
                    let owner = if failure == "clock" {
                        app.world_mut().remove_resource::<SimulationClock>()
                    } else {
                        None
                    };
                    let fixed = if failure == "fixed" {
                        app.world_mut().remove_resource::<Time<Fixed>>()
                    } else {
                        None
                    };
                    if failure == "slow-fixed" {
                        app.insert_resource(TimeUpdateStrategy::ManualDuration(
                            std::time::Duration::from_millis(1),
                        ));
                        app.world_mut()
                            .resource_mut::<Time<Fixed>>()
                            .set_timestep(std::time::Duration::from_secs(60));
                    }
                    // No catch_unwind: a Bevy missing-resource panic fails the test.
                    app.update();
                    if let Some(lab) = app.world().get_resource::<PeerLab>() {
                        assert!(!lab.is_admitting());
                    }
                    assert_eq!(
                        app.world()
                            .get_resource::<PeerLab>()
                            .map(|lab| lab.stats().game_state_updates_sent),
                        sent
                    );
                    assert_eq!(
                        app.world()
                            .resource::<Time<bevy::prelude::Virtual>>()
                            .elapsed(),
                        time
                    );
                    assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), before);
                    if clock_enabled {
                        assert!(
                            crate::network::authored_clock_runtime_health(app.world()).is_err()
                        );
                    }
                    if consumption_enabled {
                        assert!(consumption_runtime_health(app.world()).is_err());
                    }
                    assert!(capture(app.world_mut()).is_err());
                    if let Some(owner) = owner {
                        app.insert_resource(owner);
                    }
                    if let Some(fixed) = fixed {
                        app.insert_resource(fixed);
                    }
                    app.world_mut()
                        .resource_mut::<Time<Fixed>>()
                        .set_timestep(std::time::Duration::from_secs_f64(1.0 / 30.0));
                    app.update();
                    assert_eq!(app.world().resource::<SimulationClock>().tick, tick);
                    assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), before);
                    if clock_enabled {
                        assert!(
                            crate::network::authored_clock_runtime_health(app.world()).is_err()
                        );
                    }
                    if consumption_enabled {
                        assert!(consumption_runtime_health(app.world()).is_err());
                    }
                    assert!(capture(app.world_mut()).is_err());
                }
            }
        }
    }

    #[test]
    fn missing_fixed_after_first_is_caught_before_the_fixed_loop_driver() {
        let mut app = app(false, true);
        let before = app.world().resource::<PlayerRegistry>().snapshot();
        let tick = app.world().resource::<SimulationClock>().tick;
        // A later owner change must also be checked after First/TimeSystems.
        app.add_systems(bevy::prelude::PreUpdate, |world: &mut World| {
            world.remove_resource::<Time<Fixed>>();
        });
        app.update();
        assert!(consumption_runtime_health(app.world()).is_err());
        assert_eq!(app.world().resource::<SimulationClock>().tick, tick);
        assert_eq!(app.world().resource::<PlayerRegistry>().snapshot(), before);
        assert!(capture(app.world_mut()).is_err());
    }

    #[test]
    fn omitted_features_keep_normal_app_time_and_fixed_ticks() {
        let mut app = build_app(&ServerConfig::default(), &WorldSnapshot::default()).unwrap();
        app.insert_resource(TimeUpdateStrategy::FixedTimesteps(1));
        app.update();
        for _ in 0..3 {
            app.update();
        }
        assert_eq!(app.world().resource::<SimulationClock>().tick, 3);
        assert!(capture(app.world_mut()).is_ok());
        assert!(!app.world().contains_resource::<ConsumptionRuntime>());
    }
}
