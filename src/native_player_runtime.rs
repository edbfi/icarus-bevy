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
        snapshot: PlayerSnapshot,
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
        if r.revision != expected_revision {
            return Err(PlayerError::StaleRevision);
        }
        if !r.checkpoint.same_recipe(&checkpoint) {
            return Err(PlayerError::InvalidContext);
        }
        let revision = r.revision.checked_add(1).ok_or(PlayerError::Exhausted)?;
        r.checkpoint = checkpoint;
        r.revision = revision;
        Ok(r.projection(false))
    }
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
