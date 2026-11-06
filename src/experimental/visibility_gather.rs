//! Detached projection of the conditional Gather consumer in F-1023.
//!
//! Caller assertions supply the map owner, name-set source and cache lifetime;
//! callbacks do not establish them. Tokens are never dereferenced. This module
//! neither dispatches RPCs nor mutates actors, delegates or connection state.
//! Bounds and all-or-nothing publication are authored policy, not original
//! behavior. A cached-name hit deliberately bypasses supplied-name membership.

use std::collections::BTreeSet;

use super::visibility_state::{Assumption, NameIdentity, ObjectIdentity};

/// Explicit caller assertions, not discovered native bindings. Equal tokens
/// assert the same domain across calls; they prove no live object relationship.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatherBindings {
    pub graph_map_owner: Assumption<ObjectIdentity>,
    pub supplied_names_source: Assumption<ObjectIdentity>,
    pub cache_lifetime: Assumption<ObjectIdentity>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GatherRecord {
    pub name: NameIdentity,
    /// Signed eligibility predicate only, never an allocation or repeat count.
    pub count: i32,
    pub list_reference: ObjectIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub records: usize,
    pub supplied_names: usize,
    pub output_references: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GatherError {
    UnknownMapOwner,
    UnknownNamesSource,
    UnknownCacheLifetime,
    UnknownInitialName,
    BindingMismatch,
    RecordLimit,
    SuppliedNameLimit,
    OutputLimit,
}

impl GatherBindings {
    fn validate(self) -> Result<(), GatherError> {
        if matches!(self.graph_map_owner, Assumption::Unknown) {
            return Err(GatherError::UnknownMapOwner);
        }
        if matches!(self.supplied_names_source, Assumption::Unknown) {
            return Err(GatherError::UnknownNamesSource);
        }
        if matches!(self.cache_lifetime, Assumption::Unknown) {
            return Err(GatherError::UnknownCacheLifetime);
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatherState {
    limits: Limits,
    bindings: GatherBindings,
    last_name: NameIdentity,
    output: Vec<ObjectIdentity>,
}

impl GatherState {
    /// Requires an explicit initialized full name. No zero/default value or
    /// lifetime is inferred from an empty name set or a visibility callback.
    pub fn new(
        limits: Limits,
        bindings: GatherBindings,
        initial_name: Assumption<NameIdentity>,
    ) -> Result<Self, GatherError> {
        bindings.validate()?;
        let Assumption::Known(last_name) = initial_name else {
            return Err(GatherError::UnknownInitialName);
        };
        Ok(Self {
            limits,
            bindings,
            last_name,
            output: Vec::new(),
        })
    }

    pub fn last_name(&self) -> NameIdentity {
        self.last_name
    }

    pub fn output(&self) -> &[ObjectIdentity] {
        &self.output
    }

    /// Starts another caller-owned output batch without reinitializing cache.
    /// A different cache lifetime requires a new explicitly initialized state.
    pub fn clear_output(&mut self) {
        self.output.clear();
    }

    /// Consume an explicitly ordered map projection and supplied name set.
    /// Every error preserves output and cached name, including late overflow.
    /// Positive counts append one opaque list reference, preserving duplicates;
    /// no list contents are read or expanded.
    pub fn gather(
        &mut self,
        bindings: GatherBindings,
        records: &[GatherRecord],
        supplied_names: &BTreeSet<NameIdentity>,
    ) -> Result<(), GatherError> {
        bindings.validate()?;
        if bindings != self.bindings {
            return Err(GatherError::BindingMismatch);
        }
        if records.len() > self.limits.records {
            return Err(GatherError::RecordLimit);
        }
        if supplied_names.len() > self.limits.supplied_names {
            return Err(GatherError::SuppliedNameLimit);
        }
        let mut next = self.clone();
        for record in records {
            if record.count <= 0 {
                continue;
            }
            if record.name != next.last_name {
                if !supplied_names.contains(&record.name) {
                    continue;
                }
                next.last_name = record.name;
            }
            if next.output.len() >= next.limits.output_references {
                return Err(GatherError::OutputLimit);
            }
            next.output.push(record.list_reference);
        }
        *self = next;
        Ok(())
    }
}
