//! Caller-owned, inert projection of the conditional visibility contract.
//!
//! Assumptions refer only to the selected public-server 26313077 report. No
//! decoder is connected here: text/hardcoded wire names cannot qualify interned
//! identity, acceptance, native flags or receiver type. Events are intentions,
//! never calls. This models locally returning paths only, without claiming
//! original atomicity, reentrancy behavior, client readiness or compatibility.
//!
//! Destruction processing, MarkActive, graph actor bits, delegate membership,
//! compaction and channel-call implementations are deferred. The log is a
//! projection of selected ordering, not a complete execution trace.

use std::collections::{BTreeMap, BTreeSet};

/// Full interned identity within one caller-qualified name domain. Neither
/// component is inferred from a NetworkName, string suffix or hardcoded index.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct NameIdentity {
    comparison_index: u32,
    number: u32,
}

impl NameIdentity {
    pub fn caller_qualified(comparison_index: u32, number: u32) -> Self {
        Self {
            comparison_index,
            number,
        }
    }
}

/// Caller-assigned token preserving object pointer equality within this model.
/// It is never dereferenced or interpreted as an address; null is None.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct ObjectIdentity(u64);

impl ObjectIdentity {
    pub fn caller_qualified(token: u64) -> Self {
        Self(token)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Assumption<T> {
    Unknown,
    Known(T),
}

/// Independently authored classification and helper results, not loaded worlds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectDescriptor {
    InvalidOrNonLevel,
    Level {
        package: NameIdentity,
        is_persistent: Assumption<bool>,
    },
}

pub struct VisibilityFacts<'a> {
    /// Missing entries mean unknown classification, rather than non-level.
    pub objects: &'a BTreeMap<ObjectIdentity, ObjectDescriptor>,
    /// The two full names used by the selected persistent-level comparison.
    /// This connection-world fact is distinct from an add's package world.
    pub persistent_world_names: Assumption<(NameIdentity, NameIdentity)>,
}

/// An explicit receiver assertion. SelectedGraph selects only the reported
/// graph candidate; it neither discovers a live vptr nor installs delegates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReceiverProfile {
    Unknown,
    Absent,
    Opaque(ObjectIdentity),
    SelectedGraph(ObjectIdentity),
}

/// Explicit native flag policy, unavailable from decoded visible/reuse bits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RejectedClosePolicy {
    Unknown,
    NativeBit1Clear,
    NativeBit1Set,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AddAdmission {
    Unknown,
    Rejected(RejectedClosePolicy),
    Accepted {
        /// Caller-qualified package-derived world, including a known null.
        package_world: Assumption<Option<ObjectIdentity>>,
        /// Full-name graph entry lookup outcome; required only for graph add.
        graph_named_entry: Assumption<bool>,
    },
}

/// Caller-qualified actor/channel traversal result. No actor or package helper
/// runs here. The supplied slice must describe the selected traversal order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChannelDescriptor {
    pub channel: ObjectIdentity,
    pub package: NameIdentity,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VisibilityChange<'a> {
    Add {
        name: NameIdentity,
        admission: AddAdmission,
    },
    Hide {
        name: NameIdentity,
        channels: Assumption<&'a [ChannelDescriptor]>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Event {
    NameInserted {
        name: NameIdentity,
        was_new: bool,
    },
    NameRemoved {
        name: NameIdentity,
        was_present: bool,
    },
    AddCallback {
        receiver: ObjectIdentity,
        name: NameIdentity,
        package_world: Option<ObjectIdentity>,
    },
    GraphAddBroadcast {
        name: NameIdentity,
        package_world: Option<ObjectIdentity>,
        per_name: bool,
    },
    RemoveCallback {
        receiver: ObjectIdentity,
        name: NameIdentity,
    },
    GraphRemoveBroadcast {
        name: NameIdentity,
    },
    /// Unknown implementation; the only qualified channel argument is 2.
    ChannelCall {
        channel: ObjectIdentity,
        argument: u8,
    },
    /// No real close occurs. The following refresh assumes local return.
    Close,
    CacheUpdated {
        key: Option<ObjectIdentity>,
        visible: bool,
    },
    RefreshFinished,
}

/// Caller-selected native resource policy, not recovered original limits.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limits {
    pub visible_names: usize,
    pub cache_entries: usize,
    pub events: usize,
    pub channel_descriptors: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModelError {
    UnknownAcceptance,
    UnknownClosePolicy,
    UnknownReceiver,
    UnknownPackageWorld,
    UnknownGraphEntry,
    UnknownChannels,
    UnknownObjectClassification,
    UnknownPersistentLevel,
    UnknownWorldNames,
    VisibleNameLimit,
    CacheLimit,
    EventLimit,
    ChannelLimit,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VisibilityState {
    limits: Limits,
    visible_names: BTreeSet<NameIdentity>,
    cache: BTreeMap<Option<ObjectIdentity>, bool>,
    events: Vec<Event>,
}

impl VisibilityState {
    pub fn new(limits: Limits) -> Self {
        Self {
            limits,
            visible_names: BTreeSet::new(),
            cache: BTreeMap::new(),
            events: Vec::new(),
        }
    }

    pub fn contains_name(&self, name: NameIdentity) -> bool {
        self.visible_names.contains(&name)
    }

    pub fn cached(&self, key: Option<ObjectIdentity>) -> Option<bool> {
        self.cache.get(&key).copied()
    }

    pub fn events(&self) -> &[Event] {
        &self.events
    }

    pub fn clear_events(&mut self) {
        self.events.clear();
    }

    /// Native all-or-nothing admission: unknown facts or bounds preserve state
    /// and log. This guarantee is authored, not proved for the original handler.
    pub fn apply(
        &mut self,
        change: VisibilityChange<'_>,
        receiver: ReceiverProfile,
        facts: &VisibilityFacts<'_>,
    ) -> Result<(), ModelError> {
        let mut next = self.clone();
        next.apply_inner(change, receiver)?;
        next.refresh(facts)?;
        *self = next;
        Ok(())
    }

    /// Selected readiness-named predicate only. A hit bypasses classification
    /// and computation. On a miss an invalid/non-level candidate maps to null,
    /// stores true under the null key and leaves the candidate key absent.
    pub fn client_has_initialized_level_for(
        &mut self,
        candidate: Option<ObjectIdentity>,
        facts: &VisibilityFacts<'_>,
    ) -> Result<bool, ModelError> {
        if let Some(value) = self.cached(candidate) {
            return Ok(value);
        }
        let key = match candidate {
            Some(id) => match facts.objects.get(&id) {
                Some(ObjectDescriptor::Level { .. }) => Some(id),
                Some(ObjectDescriptor::InvalidOrNonLevel) => None,
                None => return Err(ModelError::UnknownObjectClassification),
            },
            None => None,
        };
        let value = self.compute(key, facts)?;
        let mut next = self.clone();
        next.update_cache(key, value)?;
        *self = next;
        Ok(value)
    }

    fn apply_inner(
        &mut self,
        change: VisibilityChange<'_>,
        receiver: ReceiverProfile,
    ) -> Result<(), ModelError> {
        match change {
            VisibilityChange::Add { name, admission } => match admission {
                AddAdmission::Unknown => return Err(ModelError::UnknownAcceptance),
                AddAdmission::Rejected(policy) => match policy {
                    RejectedClosePolicy::Unknown => return Err(ModelError::UnknownClosePolicy),
                    RejectedClosePolicy::NativeBit1Clear => self.record(Event::Close)?,
                    RejectedClosePolicy::NativeBit1Set => (),
                },
                AddAdmission::Accepted {
                    package_world,
                    graph_named_entry,
                } => {
                    if !self.contains_name(name)
                        && self.visible_names.len() >= self.limits.visible_names
                    {
                        return Err(ModelError::VisibleNameLimit);
                    }
                    let was_new = self.visible_names.insert(name);
                    self.record(Event::NameInserted { name, was_new })?;
                    match receiver {
                        ReceiverProfile::Unknown => return Err(ModelError::UnknownReceiver),
                        ReceiverProfile::Absent => (),
                        ReceiverProfile::Opaque(id) | ReceiverProfile::SelectedGraph(id) => {
                            let Assumption::Known(package_world) = package_world else {
                                return Err(ModelError::UnknownPackageWorld);
                            };
                            self.record(Event::AddCallback {
                                receiver: id,
                                name,
                                package_world,
                            })?;
                            if matches!(receiver, ReceiverProfile::SelectedGraph(_)) {
                                let Assumption::Known(per_name) = graph_named_entry else {
                                    return Err(ModelError::UnknownGraphEntry);
                                };
                                self.record(Event::GraphAddBroadcast {
                                    name,
                                    package_world,
                                    per_name: false,
                                })?;
                                if per_name {
                                    self.record(Event::GraphAddBroadcast {
                                        name,
                                        package_world,
                                        per_name: true,
                                    })?;
                                }
                            }
                        }
                    }
                }
            },
            VisibilityChange::Hide { name, channels } => {
                let Assumption::Known(channels) = channels else {
                    return Err(ModelError::UnknownChannels);
                };
                if channels.len() > self.limits.channel_descriptors {
                    return Err(ModelError::ChannelLimit);
                }
                let was_present = self.visible_names.remove(&name);
                self.record(Event::NameRemoved { name, was_present })?;
                match receiver {
                    ReceiverProfile::Unknown => return Err(ModelError::UnknownReceiver),
                    ReceiverProfile::Absent => (),
                    ReceiverProfile::Opaque(id) | ReceiverProfile::SelectedGraph(id) => {
                        self.record(Event::RemoveCallback { receiver: id, name })?;
                        if matches!(receiver, ReceiverProfile::SelectedGraph(_)) {
                            self.record(Event::GraphRemoveBroadcast { name })?;
                        }
                    }
                }
                for channel in channels.iter().filter(|channel| channel.package == name) {
                    self.record(Event::ChannelCall {
                        channel: channel.channel,
                        argument: 2,
                    })?;
                }
            }
        }
        Ok(())
    }

    fn compute(
        &self,
        key: Option<ObjectIdentity>,
        facts: &VisibilityFacts<'_>,
    ) -> Result<bool, ModelError> {
        let Some(id) = key else { return Ok(true) };
        let Some(ObjectDescriptor::Level {
            package,
            is_persistent,
        }) = facts.objects.get(&id)
        else {
            return Err(ModelError::UnknownObjectClassification);
        };
        match is_persistent {
            Assumption::Unknown => return Err(ModelError::UnknownPersistentLevel),
            Assumption::Known(false) => (),
            Assumption::Known(true) => {
                let Assumption::Known((world, connection)) = facts.persistent_world_names else {
                    return Err(ModelError::UnknownWorldNames);
                };
                if world == connection {
                    return Ok(true);
                }
            }
        }
        Ok(self.contains_name(*package))
    }

    fn refresh(&mut self, facts: &VisibilityFacts<'_>) -> Result<(), ModelError> {
        // Deterministic native order; no original cache iteration order claimed.
        // Visit only existing keys, never all caller-described world levels.
        let keys: Vec<_> = self.cache.keys().copied().flatten().collect();
        for id in keys {
            match facts.objects.get(&id) {
                None => return Err(ModelError::UnknownObjectClassification),
                Some(ObjectDescriptor::InvalidOrNonLevel) => (),
                Some(ObjectDescriptor::Level { .. }) => {
                    let value = self.compute(Some(id), facts)?;
                    self.update_cache(Some(id), value)?;
                }
            }
        }
        self.record(Event::RefreshFinished)
    }

    fn update_cache(
        &mut self,
        key: Option<ObjectIdentity>,
        visible: bool,
    ) -> Result<(), ModelError> {
        if !self.cache.contains_key(&key) && self.cache.len() >= self.limits.cache_entries {
            return Err(ModelError::CacheLimit);
        }
        self.cache.insert(key, visible);
        self.record(Event::CacheUpdated { key, visible })
    }

    fn record(&mut self, event: Event) -> Result<(), ModelError> {
        if self.events.len() >= self.limits.events {
            return Err(ModelError::EventLimit);
        }
        self.events.push(event);
        Ok(())
    }
}
