//! Experimental inventory weight cache and tick gate (F-915).
//!
//! The caller supplies both the cached weight and the calculation. This models
//! neither item/table meanings nor save restoration, tick activation or field
//! prediction. The local role belongs to the inventory's actor owner, distinct
//! from the `OwningComponent` reference used by the inventory open composer.

/// A caller-seeded cached weight, initially clean.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryWeightState {
    cached_weight: i32,
    dirty: bool,
}

impl InventoryWeightState {
    /// Seed the cache without scheduling a recalculation.
    pub fn new(cached_weight: i32) -> Self {
        Self {
            cached_weight,
            dirty: false,
        }
    }

    pub fn cached_weight(&self) -> i32 {
        self.cached_weight
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    /// Request a recalculation on a later eligible tick.
    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Recalculate only while dirty with an actor owner whose local role byte
    /// is at least 3 (unsigned, F-915). Return the new weight and clear dirty
    /// after calculation; otherwise preserve the cache and dirty state without
    /// calling `recalculate`. `None` means the actor owner is absent.
    pub fn tick(
        &mut self,
        owner_local_role: Option<u8>,
        recalculate: impl FnOnce() -> i32,
    ) -> Option<i32> {
        if !self.dirty || !owner_local_role.is_some_and(|role| role >= 3) {
            return None;
        }
        let weight = recalculate();
        self.cached_weight = weight;
        self.dirty = false;
        Some(weight)
    }
}
