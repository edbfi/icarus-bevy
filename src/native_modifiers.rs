//! Independently authored, per-owner flat modifiers and simulation-time timers.
//!
//! This contract supplies no official row/property bindings, restoration adapter,
//! activation permission, callbacks, ECS integration or persistence publication.
//! A parent must admit the live OwnerHandle, stage every affected owner, and
//! include the COMPLETE state (definitions, timers, cache, revision and ID cursor)
//! in its canonical request/snapshot validation. Reload validates current state;
//! it never reconstructs timers from a definition or catches up wall-clock time.
use std::{collections::BTreeMap, fmt, marker::PhantomData};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, de};

use crate::native_lifecycle::OwnerHandle;

pub const NATIVE_MODIFIERS_FORMAT: u32 = 1;
/// Authored policy: durations are quantized to nearest nanosecond, at most one year.
pub const MAX_MODIFIER_SECONDS: f32 = 31_536_000.0;
pub const NANOS_PER_SECOND: u64 = 1_000_000_000;
pub const MAX_MODIFIERS_PER_OWNER: usize = 64;
pub const MAX_MODIFIER_CONTRIBUTIONS: usize = 16;
pub const MAX_MODIFIER_STATS: usize = 128;
pub const MAX_MODIFIER_NAME_BYTES: usize = 128;
pub const MAX_MODIFIER_STATE_BYTES: usize = 512 * 1024;
pub const MAX_MODIFIER_OWNERS: usize = 1_024;
pub const MAX_WORLD_MODIFIERS: usize = 4_096;
pub const MAX_MODIFIER_WORLD_BYTES: usize = 8 * 1024 * 1024;

fn duration_valid(seconds: f32) -> Result<()> {
    ensure!(
        seconds.is_finite() && (0.0..=MAX_MODIFIER_SECONDS).contains(&seconds),
        "modifier duration invalid"
    );
    ensure!(
        seconds == 0.0 || f64::from(seconds) * NANOS_PER_SECOND as f64 >= 0.5,
        "modifier duration below clock resolution"
    );
    Ok(())
}

fn duration_nanos(seconds: f32) -> Result<u64> {
    duration_valid(seconds)?;
    Ok((f64::from(seconds) * NANOS_PER_SECOND as f64).round() as u64)
}

fn name_valid(name: &String) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= MAX_MODIFIER_NAME_BYTES
            && name.capacity() <= 2 * MAX_MODIFIER_NAME_BYTES
            && name.trim() == name
            && !name.chars().any(char::is_control),
        "modifier name invalid"
    );
    Ok(())
}

fn list_valid<T>(values: &Vec<T>, limit: usize) -> Result<()> {
    ensure!(
        values.len() <= limit && values.capacity() <= 2 * limit,
        "modifier collection limit"
    );
    Ok(())
}

fn decode_name<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<String, D::Error> {
    struct NameVisitor;
    impl de::Visitor<'_> for NameVisitor {
        type Value = String;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bounded modifier name")
        }
        fn visit_str<E: de::Error>(self, value: &str) -> std::result::Result<String, E> {
            if value.len() > MAX_MODIFIER_NAME_BYTES {
                return Err(E::custom("modifier name invalid"));
            }
            let name = value.to_owned();
            name_valid(&name).map_err(|_| E::custom("modifier name invalid"))?;
            Ok(name)
        }
    }
    d.deserialize_str(NameVisitor)
        .map_err(|_| de::Error::custom("modifier name invalid"))
}

fn decode_list<'de, D, T, const N: usize>(d: D) -> std::result::Result<Vec<T>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    struct ListVisitor<T, const N: usize>(PhantomData<T>);
    impl<'de, T: Deserialize<'de>, const N: usize> de::Visitor<'de> for ListVisitor<T, N> {
        type Value = Vec<T>;
        fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str("a bounded modifier list")
        }
        fn visit_seq<A: de::SeqAccess<'de>>(
            self,
            mut seq: A,
        ) -> std::result::Result<Vec<T>, A::Error> {
            if seq.size_hint().is_some_and(|size| size > N) {
                return Err(de::Error::custom("modifier collection limit"));
            }
            let mut values = Vec::new();
            while let Some(value) = seq.next_element()? {
                if values.len() == N {
                    return Err(de::Error::custom("modifier collection limit"));
                }
                values.push(value);
            }
            Ok(values)
        }
    }
    d.deserialize_seq(ListVisitor::<T, N>(PhantomData))
        .map_err(|_| de::Error::custom("modifier collection invalid"))
}

// Objects reject positional sequences, duplicate/missing/unknown fields and
// semantic failures. Error chains and Debug never include caller-supplied names.
macro_rules! dto {
    ($name:ident { $($(#[$attr:meta])* pub $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Clone, PartialEq, Serialize)]
        pub struct $name { $(pub $field: $ty),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                #[derive(Deserialize)]
                #[serde(deny_unknown_fields)]
                struct Fields { $($(#[$attr])* $field: $ty),* }
                let fields: Fields = crate::json::object(d)
                    .map_err(|_| <D::Error as de::Error>::custom("modifier object invalid"))?;
                let result = Self { $($field: fields.$field),* };
                result.validate()
                    .map_err(|_| <D::Error as de::Error>::custom("modifier object invalid"))?;
                Ok(result)
            }
        }
        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.debug_struct(stringify!($name)).finish_non_exhaustive()
            }
        }
    };
}

/// Positive, owner-local identity. The persisted monotonic cursor prevents reuse
/// after removal/expiry within an owner incarnation.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct ModifierId(u64);
impl ModifierId {
    pub fn new(value: u64) -> Result<Self> {
        ensure!(value != 0, "modifier identity invalid");
        Ok(Self(value))
    }
    pub fn get(self) -> u64 {
        self.0
    }
}
impl fmt::Debug for ModifierId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ModifierId(..)")
    }
}
impl<'de> Deserialize<'de> for ModifierId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = u64::deserialize(d)
            .map_err(|_| <D::Error as de::Error>::custom("modifier identity invalid"))?;
        Self::new(value).map_err(|_| de::Error::custom("modifier identity invalid"))
    }
}

/// Authored policy only. Zero is valid and creates an unarmed timed entry.
#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModifierDuration {
    Indefinite,
    Timed { seconds: f32 },
}
impl ModifierDuration {
    pub fn validate(&self) -> Result<()> {
        if let Self::Timed { seconds } = self {
            duration_valid(*seconds)?;
        }
        Ok(())
    }
}
impl fmt::Debug for ModifierDuration {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ModifierDuration(..)")
    }
}
impl<'de> Deserialize<'de> for ModifierDuration {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
        enum Fields {
            Indefinite {},
            Timed { seconds: f32 },
        }
        let value = match crate::json::object(d)
            .map_err(|_| <D::Error as de::Error>::custom("modifier policy invalid"))?
        {
            Fields::Indefinite {} => Self::Indefinite,
            Fields::Timed { seconds } => Self::Timed { seconds },
        };
        value
            .validate()
            .map_err(|_| <D::Error as de::Error>::custom("modifier policy invalid"))?;
        Ok(value)
    }
}

/// Current timer state is retained verbatim across reload. Positive timed
/// entries are armed; zero timed entries are unarmed and never auto-expire.
#[derive(Clone, Copy, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModifierTimer {
    Indefinite,
    Timed { remaining_nanos: u64, armed: bool },
}
impl ModifierTimer {
    pub fn validate(&self) -> Result<()> {
        if let Self::Timed {
            remaining_nanos,
            armed,
        } = self
        {
            ensure!(
                *remaining_nanos <= duration_nanos(MAX_MODIFIER_SECONDS)?,
                "modifier timer duration invalid"
            );
            ensure!(
                *armed == (*remaining_nanos > 0),
                "modifier timer arm invalid"
            );
        }
        Ok(())
    }
}
impl fmt::Debug for ModifierTimer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ModifierTimer(..)")
    }
}
impl<'de> Deserialize<'de> for ModifierTimer {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
        enum Fields {
            Indefinite {},
            Timed { remaining_nanos: u64, armed: bool },
        }
        let value = match crate::json::object(d)
            .map_err(|_| <D::Error as de::Error>::custom("modifier timer invalid"))?
        {
            Fields::Indefinite {} => Self::Indefinite,
            Fields::Timed {
                remaining_nanos,
                armed,
            } => Self::Timed {
                remaining_nanos,
                armed,
            },
        };
        value
            .validate()
            .map_err(|_| <D::Error as de::Error>::custom("modifier timer invalid"))?;
        Ok(value)
    }
}

dto!(FlatStatContribution {
    #[serde(deserialize_with = "decode_name")]
    pub stat: String,
    pub amount: i32,
});
impl FlatStatContribution {
    pub fn validate(&self) -> Result<()> {
        name_valid(&self.stat)
    }
}

dto!(ModifierDefinition {
    #[serde(deserialize_with = "decode_name")]
    pub name: String,
    pub duration: ModifierDuration,
    #[serde(deserialize_with = "decode_list::<_, _, MAX_MODIFIER_CONTRIBUTIONS>")]
    pub contributions: Vec<FlatStatContribution>,
});
impl ModifierDefinition {
    pub fn validate(&self) -> Result<()> {
        name_valid(&self.name)?;
        self.duration.validate()?;
        list_valid(&self.contributions, MAX_MODIFIER_CONTRIBUTIONS)?;
        let mut names = std::collections::BTreeSet::new();
        for contribution in &self.contributions {
            contribution.validate()?;
            ensure!(
                names.insert(&contribution.stat),
                "modifier duplicate stat name"
            );
        }
        Ok(())
    }
    pub fn canonical(&self) -> Result<Self> {
        self.validate()?;
        let mut value = self.clone();
        value.contributions.sort_by(|a, b| a.stat.cmp(&b.stat));
        Ok(value)
    }
}

dto!(ModifierEntry {
    pub id: ModifierId,
    pub definition: ModifierDefinition,
    pub timer: ModifierTimer,
});
impl ModifierEntry {
    pub fn validate(&self) -> Result<()> {
        self.definition.validate()?;
        self.timer.validate()?;
        match (self.definition.duration, self.timer) {
            (ModifierDuration::Indefinite, ModifierTimer::Indefinite) => {}
            (
                ModifierDuration::Timed { seconds },
                ModifierTimer::Timed {
                    remaining_nanos, ..
                },
            ) => {
                ensure!(
                    remaining_nanos <= duration_nanos(seconds)?,
                    "modifier remaining duration invalid"
                );
            }
            _ => anyhow::bail!("modifier timer policy mismatch"),
        }
        Ok(())
    }
}

dto!(OwnerModifierState {
    pub format: u32,
    pub owner: OwnerHandle,
    pub revision: u64,
    pub next_id: u64,
    #[serde(deserialize_with = "decode_list::<_, _, MAX_MODIFIERS_PER_OWNER>")]
    pub entries: Vec<ModifierEntry>,
    #[serde(deserialize_with = "decode_list::<_, _, MAX_MODIFIER_STATS>")]
    pub cache: Vec<FlatStatContribution>,
});
impl OwnerModifierState {
    /// The caller supplies an already admitted live owner; no activation is
    /// implied. New incarnations receive a new state, never an old timer set.
    pub fn empty(owner: OwnerHandle) -> Result<Self> {
        let state = Self {
            format: NATIVE_MODIFIERS_FORMAT,
            owner,
            revision: 0,
            next_id: 1,
            entries: Vec::new(),
            cache: Vec::new(),
        };
        state.validate()?;
        Ok(state)
    }

    /// Complete semantic/capacity validation, including the CURRENT timer and
    /// exact cached modifier aggregate (base stats are outside this contract).
    /// Zero totals remain named until the last contributing entry leaves.
    /// Validation never repairs or restarts state.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.format == NATIVE_MODIFIERS_FORMAT,
            "modifier format invalid"
        );
        owner_valid(self.owner)?;
        ensure!(self.next_id > 0, "modifier identity cursor invalid");
        list_valid(&self.entries, MAX_MODIFIERS_PER_OWNER)?;
        list_valid(&self.cache, MAX_MODIFIER_STATS)?;
        let mut names = std::collections::BTreeSet::new();
        let mut prior_id = 0;
        for entry in &self.entries {
            entry.validate()?;
            ensure!(
                prior_id < entry.id.get() && entry.id.get() < self.next_id,
                "modifier identity order or cursor invalid"
            );
            prior_id = entry.id.get();
            ensure!(
                names.insert(&entry.definition.name),
                "modifier duplicate definition name"
            );
        }
        for value in &self.cache {
            value.validate()?;
        }
        ensure!(
            self.cache == aggregate(&self.entries)?,
            "modifier cache invalid"
        );
        Ok(())
    }

    pub(crate) fn allocation_bytes(&self) -> Result<usize> {
        self.validate()?;
        let mut bytes = self.entries.capacity() * std::mem::size_of::<ModifierEntry>()
            + self.cache.capacity() * std::mem::size_of::<FlatStatContribution>();
        for value in &self.cache {
            bytes += value.stat.capacity();
        }
        for entry in &self.entries {
            bytes += entry.definition.name.capacity();
            bytes += entry.definition.contributions.capacity()
                * std::mem::size_of::<FlatStatContribution>();
            for value in &entry.definition.contributions {
                bytes += value.stat.capacity();
            }
        }
        Ok(bytes)
    }

    /// Normalizes definition contribution order only, retaining all current
    /// timers/cache/IDs. Entries and cache must already have canonical ordering.
    pub fn canonical(&self) -> Result<Self> {
        self.validate()?;
        let mut state = self.clone();
        for entry in &mut state.entries {
            entry.definition = entry.definition.canonical()?;
        }
        Ok(state)
    }

    /// Bounded, redacted in-memory JSON decode; no filesystem/publication API.
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_MODIFIER_STATE_BYTES,
            "modifier state byte limit"
        );
        serde_json::from_str(text).map_err(|_| anyhow::anyhow!("modifier state JSON invalid"))
    }
}

fn owner_valid(owner: OwnerHandle) -> Result<()> {
    ensure!(
        owner.id.0 > 0 && owner.incarnation > 0,
        "modifier owner invalid"
    );
    Ok(())
}

// Aggregate in i64, then check the final i32 range. This admits the entire
// signed contribution domain, including MIN, and order-independent cancellation.
// No negation/subtraction of MIN and no cross-owner search is performed.
fn aggregate(entries: &[ModifierEntry]) -> Result<Vec<FlatStatContribution>> {
    let mut totals: BTreeMap<&str, i64> = BTreeMap::new();
    for entry in entries {
        for value in &entry.definition.contributions {
            let total = totals.entry(value.stat.as_str()).or_default();
            *total = total
                .checked_add(i64::from(value.amount))
                .context("modifier stat overflow")?;
            ensure!(totals.len() <= MAX_MODIFIER_STATS, "modifier stat capacity");
        }
    }
    totals
        .into_iter()
        .map(|(stat, amount)| {
            Ok(FlatStatContribution {
                stat: stat.to_owned(),
                amount: i32::try_from(amount).context("modifier stat overflow")?,
            })
        })
        .collect()
}

#[derive(Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ModifierAction {
    Add { definition: ModifierDefinition },
    Remove { id: ModifierId },
    Advance { elapsed_nanos: u64 },
}
impl ModifierAction {
    pub fn validate(&self) -> Result<()> {
        match self {
            Self::Add { definition } => definition.validate(),
            Self::Remove { .. } => Ok(()),
            Self::Advance { .. } => Ok(()),
        }
    }
}
impl fmt::Debug for ModifierAction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ModifierAction(..)")
    }
}
impl<'de> Deserialize<'de> for ModifierAction {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
        enum Fields {
            Add { definition: ModifierDefinition },
            Remove { id: ModifierId },
            Advance { elapsed_nanos: u64 },
        }
        let value = match crate::json::object(d)
            .map_err(|_| <D::Error as de::Error>::custom("modifier action invalid"))?
        {
            Fields::Add { definition } => Self::Add { definition },
            Fields::Remove { id } => Self::Remove { id },
            Fields::Advance { elapsed_nanos } => Self::Advance { elapsed_nanos },
        };
        value
            .validate()
            .map_err(|_| <D::Error as de::Error>::custom("modifier action invalid"))?;
        Ok(value)
    }
}

dto!(ModifierRequest {
    pub owner: OwnerHandle,
    pub expected_revision: u64,
    pub action: ModifierAction,
});
impl ModifierRequest {
    pub fn validate(&self) -> Result<()> {
        owner_valid(self.owner)?;
        self.action.validate()
    }
    pub fn canonical(&self) -> Result<Self> {
        self.validate()?;
        let mut request = self.clone();
        if let ModifierAction::Add { definition } = &mut request.action {
            *definition = definition.canonical()?;
        }
        Ok(request)
    }
}

/// Immutable staging token. It retains the complete pre-state so application
/// checks more than a revision. It neither borrows nor mutates the active world.
pub struct PreparedModifierChange {
    before: OwnerModifierState,
    after: OwnerModifierState,
    added: Option<ModifierId>,
    expired: Vec<ModifierId>,
}
impl fmt::Debug for PreparedModifierChange {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedModifierChange")
            .field("entries", &self.after.entries.len())
            .field("expired", &self.expired.len())
            .finish_non_exhaustive()
    }
}
impl PreparedModifierChange {
    pub fn state(&self) -> &OwnerModifierState {
        &self.after
    }
    pub fn added_id(&self) -> Option<ModifierId> {
        self.added
    }
    pub fn expired_ids(&self) -> &[ModifierId] {
        &self.expired
    }
    /// Return the replacement only if the complete current state still matches
    /// preparation, including exact owner incarnation and revision. The parent
    /// must validate liveness against its owner registry before committing.
    pub fn apply(self, current: &OwnerModifierState) -> Result<OwnerModifierState> {
        current.validate()?;
        ensure!(*current == self.before, "modifier prepared state stale");
        Ok(self.after)
    }
}

/// Prepare one owner operation without changing any caller state. Capacity,
/// duration, stat and counter failures expose no partially updated state.
/// Successful no-ops keep the revision; any actual change increments it once.
pub fn prepare_modifiers(
    state: &OwnerModifierState,
    request: &ModifierRequest,
) -> Result<PreparedModifierChange> {
    state.validate()?;
    let request = request.canonical()?;
    ensure!(request.owner == state.owner, "modifier owner stale");
    ensure!(
        request.expected_revision == state.revision,
        "modifier revision stale"
    );
    let mut after = state.clone();
    let mut added = None;
    let mut expired = Vec::new();
    match request.action {
        ModifierAction::Add { definition } => {
            ensure!(
                after.entries.len() < MAX_MODIFIERS_PER_OWNER,
                "modifier entry capacity"
            );
            ensure!(
                !after
                    .entries
                    .iter()
                    .any(|entry| entry.definition.name == definition.name),
                "modifier duplicate definition name"
            );
            let next_id = after
                .next_id
                .checked_add(1)
                .context("modifier identity exhausted")?;
            let id = ModifierId::new(after.next_id)?;
            let timer = match definition.duration {
                ModifierDuration::Indefinite => ModifierTimer::Indefinite,
                ModifierDuration::Timed { seconds } => ModifierTimer::Timed {
                    remaining_nanos: duration_nanos(seconds)?,
                    armed: seconds > 0.0,
                },
            };
            after.entries.push(ModifierEntry {
                id,
                definition,
                timer,
            });
            after.next_id = next_id;
            added = Some(id);
        }
        ModifierAction::Remove { id } => {
            let index = after
                .entries
                .binary_search_by_key(&id, |entry| entry.id)
                .map_err(|_| anyhow::anyhow!("modifier entry missing"))?;
            after.entries.remove(index);
        }
        ModifierAction::Advance { elapsed_nanos } => {
            after.entries.retain_mut(|entry| {
                if let ModifierTimer::Timed {
                    remaining_nanos,
                    armed: true,
                } = &mut entry.timer
                {
                    if elapsed_nanos >= *remaining_nanos {
                        expired.push(entry.id);
                        return false;
                    }
                    *remaining_nanos -= elapsed_nanos;
                }
                true
            });
        }
    }
    after.cache = aggregate(&after.entries)?;
    if after != *state {
        after.revision = state
            .revision
            .checked_add(1)
            .context("modifier revision exhausted")?;
    }
    after.validate()?;
    Ok(PreparedModifierChange {
        before: state.clone(),
        after,
        added,
        expired,
    })
}

/// Complete authored modifier state for every live lifecycle owner. Dormant
/// slots carry no modifier state; a new incarnation starts with an empty state.
#[derive(Clone, PartialEq, Serialize)]
pub struct ModifierWorldSnapshot {
    pub format: u32,
    pub owners: Vec<OwnerModifierState>,
}
impl fmt::Debug for ModifierWorldSnapshot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ModifierWorldSnapshot")
            .field("owners", &self.owners.len())
            .finish_non_exhaustive()
    }
}
impl<'de> Deserialize<'de> for ModifierWorldSnapshot {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            format: u32,
            #[serde(deserialize_with = "decode_list::<_, _, MAX_MODIFIER_OWNERS>")]
            owners: Vec<OwnerModifierState>,
        }
        let fields: Fields = crate::json::object(d)
            .map_err(|_| <D::Error as de::Error>::custom("modifier world object invalid"))?;
        let value = Self {
            format: fields.format,
            owners: fields.owners,
        };
        value
            .validate_structure()
            .map_err(|_| de::Error::custom("modifier world invalid"))?;
        Ok(value)
    }
}
impl ModifierWorldSnapshot {
    /// Charge caller-owned spare capacities before cloning any owner payload.
    pub fn validate_structure(&self) -> Result<()> {
        ensure!(
            self.format == NATIVE_MODIFIERS_FORMAT,
            "modifier world format invalid"
        );
        list_valid(&self.owners, MAX_MODIFIER_OWNERS)?;
        let mut bytes = self.owners.capacity() * std::mem::size_of::<OwnerModifierState>();
        let mut entries = 0usize;
        let mut previous = 0;
        for owner in &self.owners {
            owner.validate()?;
            ensure!(owner.owner.id.0 > previous, "modifier owner order invalid");
            previous = owner.owner.id.0;
            entries += owner.entries.len();
            ensure!(entries <= MAX_WORLD_MODIFIERS, "modifier world entry limit");
            bytes += owner.allocation_bytes()?;
            ensure!(
                bytes <= MAX_MODIFIER_WORLD_BYTES,
                "modifier world allocation limit"
            );
        }
        ensure!(
            bytes <= MAX_MODIFIER_WORLD_BYTES,
            "modifier world allocation limit"
        );
        Ok(())
    }
    pub fn validate(&self, lifecycle: &crate::native_lifecycle::LifecycleSnapshot) -> Result<()> {
        self.validate_structure()?;
        let mut live: Vec<_> = lifecycle
            .slots
            .iter()
            .filter(|slot| slot.presence == crate::native_lifecycle::Presence::Live)
            .map(|slot| OwnerHandle {
                id: slot.id,
                incarnation: slot.incarnation,
            })
            .collect();
        live.sort_by_key(|owner| owner.id);
        ensure!(
            live.len() == self.owners.len()
                && live
                    .iter()
                    .zip(&self.owners)
                    .all(|(expected, actual)| *expected == actual.owner),
            "modifier live owner membership invalid"
        );
        Ok(())
    }
    /// Called only after the old complete state has been admitted. Retain exact
    /// current state for unchanged handles, drop retired incarnations, and create
    /// empty state for newly live handles. This never replays an add operation.
    pub(crate) fn reconcile(
        &self,
        lifecycle: &crate::native_lifecycle::LifecycleSnapshot,
    ) -> Result<Self> {
        self.validate_structure()?;
        let mut owners = Vec::new();
        for slot in &lifecycle.slots {
            if slot.presence != crate::native_lifecycle::Presence::Live {
                continue;
            }
            ensure!(owners.len() < MAX_MODIFIER_OWNERS, "modifier owner limit");
            let handle = OwnerHandle {
                id: slot.id,
                incarnation: slot.incarnation,
            };
            let current = self
                .owners
                .binary_search_by_key(&slot.id, |owner| owner.owner.id)
                .ok()
                .map(|index| &self.owners[index])
                .filter(|owner| owner.owner == handle);
            owners.push(match current {
                Some(owner) => owner.clone(),
                None => OwnerModifierState::empty(handle)?,
            });
        }
        owners.sort_by_key(|owner| owner.owner.id);
        let result = Self {
            format: NATIVE_MODIFIERS_FORMAT,
            owners,
        };
        result.validate(lifecycle)?;
        Ok(result)
    }
}

/// Explicit opt-in upgrade of an independently authored schema-7 world. Other
/// modes, including every C001 fixture/import mode, have no implicit upgrade.
pub fn enable_modifier_owners(
    mut snapshot: crate::simulation::WorldSnapshot,
) -> Result<crate::simulation::WorldSnapshot> {
    ensure!(
        snapshot.schema_version == crate::simulation::SNAPSHOT_SCHEMA_LIFECYCLE,
        "modifier opt-in requires authored lifecycle schema 7"
    );
    snapshot.validate()?;
    let empty = ModifierWorldSnapshot {
        format: NATIVE_MODIFIERS_FORMAT,
        owners: Vec::new(),
    };
    let state = empty.reconcile(
        snapshot
            .native_lifecycle
            .as_ref()
            .context("modifier lifecycle missing")?,
    )?;
    snapshot.schema_version = crate::simulation::SNAPSHOT_SCHEMA_MODIFIER_OWNERS;
    snapshot.native_modifiers = Some(state);
    snapshot.validate()?;
    Ok(snapshot)
}
