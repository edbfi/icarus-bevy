//! Independently authored stacked inventories. No original item-operation
//! semantics, source interpretation, lifecycle execution, or persistence.
//!
//! Conservation validates a state against supplied immutable initial totals;
//! it is not a cryptographic history proof. The composing caller pins provenance.
use crate::{
    experimental::restore_rules::{
        InventorySlotOverflowDisposition, resolve_inventory_slot_restore,
    },
    native_lifecycle::OwnerHandle,
    native_world::CompiledNativeCatalog,
    simulation::DomainId,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Deserializer, Serialize, de};
use sha2::{Digest, Sha256};
use std::{
    cell::Cell,
    collections::{BTreeMap, BTreeSet},
    fmt,
    fs::OpenOptions,
    io::{Read, Write},
    marker::PhantomData,
    mem::{size_of, size_of_val},
    os::unix::fs::OpenOptionsExt,
    path::Path,
    sync::Arc,
};

pub const MAX_INVENTORY_CONTRACT_BYTES: usize = 4 * 1024 * 1024;
pub const MAX_INVENTORY_STATE_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_INVENTORY_GRAPH_BYTES: usize = 64 * 1024 * 1024;
pub const MAX_INVENTORY_OWNERS: usize = 16384;
pub const MAX_INVENTORY_ITEMS: usize = 1024;
pub const MAX_INVENTORY_INSTANCES: usize = 65536;
pub const MAX_INVENTORY_STACKS: usize = 65536;
pub const MAX_INVENTORY_STACK_COUNT: u32 = 1_000_000;

#[derive(Default)]
struct Budget(usize);
impl Budget {
    fn add(&mut self, n: usize) -> Result<()> {
        self.0 = self
            .0
            .checked_add(n)
            .filter(|v| *v <= MAX_INVENTORY_GRAPH_BYTES)
            .ok_or_else(|| anyhow::anyhow!("inventory allocation limit"))?;
        Ok(())
    }
    fn array<T>(&mut self, n: usize) -> Result<()> {
        self.add(
            n.checked_mul(size_of::<T>())
                .ok_or_else(|| anyhow::anyhow!("inventory allocation limit"))?,
        )
    }
    // Admission covers the caller's allocation AND the capacity needed to
    // reload its encoding. In particular, len == 0 never hides spare storage.
    fn vector<T: Graph>(&mut self, len: usize, capacity: usize) -> Result<()> {
        self.array::<T>(capacity.max(decode_capacity::<T>(len)?))
    }
}
trait Graph {
    const LIMIT: usize;
    fn walk(&self, budget: &mut Budget) -> Result<()>;
}
fn next_decode_capacity<T: Graph>(capacity: usize) -> Result<usize> {
    capacity
        .checked_mul(2)
        .map(|n| n.max(4).min(T::LIMIT))
        .filter(|n| *n > capacity)
        .ok_or_else(|| anyhow::anyhow!("inventory sequence limit"))
}
fn decode_capacity<T: Graph>(len: usize) -> Result<usize> {
    ensure!(len <= T::LIMIT, "inventory sequence limit");
    let mut capacity = 0;
    while capacity < len {
        capacity = next_decode_capacity::<T>(capacity)?;
    }
    Ok(capacity)
}
impl Graph for String {
    const LIMIT: usize = MAX_INVENTORY_ITEMS;
    fn walk(&self, b: &mut Budget) -> Result<()> {
        ensure!(
            !self.is_empty()
                && self.len() <= 64
                && self.capacity() <= 132
                && self
                    .bytes()
                    .all(|v| v.is_ascii_alphanumeric() || b"-_.".contains(&v)),
            "inventory key invalid"
        );
        b.add(self.capacity())
    }
}
impl<T: Graph> Graph for Vec<T> {
    const LIMIT: usize = T::LIMIT;
    fn walk(&self, b: &mut Budget) -> Result<()> {
        ensure!(
            self.len() <= T::LIMIT && self.capacity() <= 2 * T::LIMIT + 4,
            "inventory sequence limit"
        );
        b.vector::<T>(self.len(), self.capacity())?;
        for v in self {
            v.walk(b)?;
        }
        Ok(())
    }
}
impl<T: Graph> Graph for Option<T> {
    const LIMIT: usize = T::LIMIT;
    fn walk(&self, b: &mut Budget) -> Result<()> {
        if let Some(v) = self {
            v.walk(b)?;
        }
        Ok(())
    }
}
fn graph(value: &impl Graph) -> Result<usize> {
    let mut b = Budget::default();
    b.add(size_of_val(value))?;
    value.walk(&mut b)?;
    Ok(b.0)
}

// Decode admission is shared by nested objects, with no retained per-thread
// allowance after the root exits (also on malformed input).
thread_local! {
    static DECODE_DEPTH: Cell<usize> = const { Cell::new(0) };
    static DECODE_BYTES: Cell<usize> = const { Cell::new(0) };
    static DECODE_STACKS: Cell<usize> = const { Cell::new(0) };
    static DECODE_INSTANCES: Cell<usize> = const { Cell::new(0) };
}
pub(crate) struct InventoryDecodeScope;
impl InventoryDecodeScope {
    pub(crate) fn enter() -> Self {
        DECODE_DEPTH.with(|d| {
            if d.get() == 0 {
                DECODE_BYTES.with(|b| b.set(0));
                DECODE_STACKS.with(|b| b.set(0));
                DECODE_INSTANCES.with(|b| b.set(0));
            }
            d.set(d.get() + 1);
        });
        Self
    }
}
impl Drop for InventoryDecodeScope {
    fn drop(&mut self) {
        DECODE_DEPTH.with(|d| d.set(d.get() - 1));
    }
}
fn charge<E: de::Error>(n: usize) -> std::result::Result<(), E> {
    DECODE_BYTES.with(|b| {
        let v = b
            .get()
            .checked_add(n)
            .filter(|v| *v <= MAX_INVENTORY_GRAPH_BYTES)
            .ok_or_else(|| E::custom("inventory decode allocation limit"))?;
        b.set(v);
        Ok(())
    })
}
fn admit_object<E: de::Error>(name: &str) -> std::result::Result<(), E> {
    let increment = |cell: &Cell<usize>, limit: usize| {
        let n = cell
            .get()
            .checked_add(1)
            .filter(|v| *v <= limit)
            .ok_or_else(|| E::custom("inventory decode aggregate limit"))?;
        cell.set(n);
        Ok(())
    };
    match name {
        "StackV1" | "InitialInventoryItemV1" => {
            DECODE_STACKS.with(|c| increment(c, MAX_INVENTORY_STACKS))
        }
        "InventoryStateV1" => DECODE_INSTANCES.with(|c| increment(c, MAX_INVENTORY_INSTANCES)),
        _ => Ok(()),
    }
}
trait Field: Sized {
    fn field<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error>;
}
fn field<'de, D: Deserializer<'de>, T: Field>(d: D) -> std::result::Result<T, D::Error> {
    T::field(d)
}
impl Field for String {
    fn field<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl de::Visitor<'_> for V {
            type Value = String;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("inventory string")
            }
            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<String, E> {
                if v.is_empty()
                    || v.len() > 64
                    || !v
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
                {
                    return Err(E::custom("inventory string limit"));
                }
                charge::<E>(v.len())?;
                Ok(v.to_owned())
            }
        }
        d.deserialize_str(V)
    }
}
fn finish_decode<T: Serialize + Graph, E: de::Error>(
    name: &str,
    value: &T,
) -> std::result::Result<(), E> {
    if DECODE_DEPTH.with(|d| d.get() == 1) {
        graph(value).map_err(|_| E::custom("inventory object limit"))?;
    }
    let limit = match name {
        "InventoryContractsV1" | "InventoryContractsSnapshotV1" => MAX_INVENTORY_CONTRACT_BYTES,
        "InventoryStateSnapshotV1" => MAX_INVENTORY_STATE_BYTES,
        "InitialInventoryItemsV1" | "InventoryRestoreRequestV1" => MAX_INVENTORY_GRAPH_BYTES,
        _ => return Ok(()),
    };
    encoded(value, limit).map_err(|_| E::custom("inventory object limit"))?;
    Ok(())
}
impl<T: Field + Graph> Field for Vec<T> {
    fn field<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V<T>(PhantomData<T>);
        struct Seed<T>(usize, PhantomData<T>);
        impl<'de, T: Field + Graph> de::DeserializeSeed<'de> for Seed<T> {
            type Value = T;
            fn deserialize<D: Deserializer<'de>>(self, d: D) -> std::result::Result<T, D::Error> {
                if self.0 >= T::LIMIT {
                    return Err(de::Error::custom("inventory sequence limit"));
                }
                T::field(d)
            }
        }
        impl<'de, T: Field + Graph> de::Visitor<'de> for V<T> {
            type Value = Vec<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded inventory sequence")
            }
            fn visit_seq<A: de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Vec<T>, A::Error> {
                let mut v = Vec::new();
                while let Some(item) = a.next_element_seed(Seed::<T>(v.len(), PhantomData))? {
                    if v.len() == v.capacity() {
                        let cap = next_decode_capacity::<T>(v.capacity()).map_err(|_| {
                            <A::Error as de::Error>::custom("inventory sequence limit")
                        })?;
                        charge::<A::Error>(
                            (cap - v.capacity())
                                .checked_mul(size_of::<T>())
                                .ok_or_else(|| {
                                    <A::Error as de::Error>::custom("inventory allocation limit")
                                })?,
                        )?;
                        v.try_reserve_exact(cap - v.len()).map_err(|_| {
                            <A::Error as de::Error>::custom("inventory allocation limit")
                        })?;
                    }
                    v.push(item);
                }
                Ok(v)
            }
        }
        d.deserialize_seq(V(PhantomData))
    }
}
impl<T: Field> Field for Option<T> {
    fn field<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V<T>(PhantomData<T>);
        impl<'de, T: Field> de::Visitor<'de> for V<T> {
            type Value = Option<T>;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("optional inventory stack")
            }
            fn visit_none<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_unit<E: de::Error>(self) -> std::result::Result<Self::Value, E> {
                Ok(None)
            }
            fn visit_some<D: Deserializer<'de>>(
                self,
                d: D,
            ) -> std::result::Result<Self::Value, D::Error> {
                T::field(d).map(Some)
            }
        }
        d.deserialize_option(V(PhantomData))
    }
}
macro_rules! leaf {
    ($t:ty, $limit:expr) => {
        impl Graph for $t {
            const LIMIT: usize = $limit;
            fn walk(&self, _: &mut Budget) -> Result<()> {
                Ok(())
            }
        }
        impl Field for $t {
            fn field<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                Self::deserialize(d)
            }
        }
    };
}
leaf!(u16, 256);
leaf!(u32, 1024);
leaf!(u64, 1024);
leaf!(bool, 1024);
leaf!(DomainId, MAX_INVENTORY_OWNERS);
leaf!(OwnerHandle, MAX_INVENTORY_OWNERS);

macro_rules! dto {
    ($name:ident, $limit:expr, { $(pub $field:ident: $ty:ty),* $(,)? }) => {
        #[derive(Clone, PartialEq, Serialize)]
        pub struct $name { $(pub $field:$ty),* }
        impl<'de> Deserialize<'de> for $name {
            fn deserialize<D:Deserializer<'de>>(d:D)->std::result::Result<Self,D::Error> {
                let _scope=InventoryDecodeScope::enter();
                admit_object::<D::Error>(stringify!($name))?;
                #[derive(Deserialize)] #[serde(deny_unknown_fields)]
                struct Fields { $(#[serde(deserialize_with="field")] $field:$ty),* }
                let v:Fields=crate::json::object(d).map_err(|_| <D::Error as de::Error>::custom("inventory object invalid"))?;
                let result=Self { $($field:v.$field),* };
                finish_decode::<Self,D::Error>(stringify!($name),&result)?;
                Ok(result)
            }
        }
        impl Field for $name { fn field<'de,D:Deserializer<'de>>(d:D)->std::result::Result<Self,D::Error> { Self::deserialize(d) } }
        impl Graph for $name {
            const LIMIT:usize=$limit;
            fn walk(&self,b:&mut Budget)->Result<()> { $(self.$field.walk(b)?;)* Ok(()) }
        }
        impl fmt::Debug for $name {
            fn fmt(&self,f:&mut fmt::Formatter<'_>)->fmt::Result { f.debug_struct(stringify!($name)).finish_non_exhaustive() }
        }
    };
}

/// Positive world-local identity. Retired identities are never allocated again.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct StackId(u64);
impl StackId {
    pub fn new(value: u64) -> Result<Self> {
        ensure!(value > 0, "inventory stack identity invalid");
        Ok(Self(value))
    }
    pub fn get(self) -> u64 {
        self.0
    }
}
impl fmt::Debug for StackId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("StackId(..)")
    }
}
impl<'de> Deserialize<'de> for StackId {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let value = u64::deserialize(d)
            .map_err(|_| <D::Error as de::Error>::custom("inventory stack identity invalid"))?;
        if value == 0 {
            return Err(de::Error::custom("inventory stack identity invalid"));
        }
        Ok(Self(value))
    }
}
leaf!(StackId, 256);
dto!(ItemDefinitionV1,1024,{pub key:String,pub stack_limit:u32});
dto!(SlotRuleV1,256,{pub allowed_items:Vec<String>});
dto!(InventoryDefinitionV1,8,{pub key:String,pub initial_slots:u16,pub slot_rules:Vec<SlotRuleV1>,pub force_add_when_zero:bool,pub overflow_limit:u16});
dto!(InventoryOwnerClassV1,16384,{pub native_class:String,pub inventories:Vec<InventoryDefinitionV1>});
dto!(InventoryContractsV1,1,{pub format:u32,pub native_catalog_id:String,pub native_map:String,pub items:Vec<ItemDefinitionV1>,pub classes:Vec<InventoryOwnerClassV1>});
dto!(InventoryContractsSnapshotV1,1,{pub contract_id:String,pub definition:InventoryContractsV1});
dto!(StackV1,256,{pub id:StackId,pub item:String,pub count:u32});
dto!(DeferredStackV1,256,{pub stack:StackV1,pub requested_slot:u16});
dto!(InventoryStateV1,8,{pub key:String,pub slots:Vec<Option<StackV1>>,pub overflow:Vec<DeferredStackV1>});
dto!(OwnerInventoriesV1,16384,{pub owner:OwnerHandle,pub inventories:Vec<InventoryStateV1>});
dto!(ItemTotalV1,1024,{pub item:String,pub count:u64});
dto!(InventoryStateSnapshotV1,1,{pub format:u32,pub contract_id:String,pub revision:u64,pub next_stack_id:u64,pub consumed:Vec<ItemTotalV1>,pub owners:Vec<OwnerInventoriesV1>});
dto!(InitialInventoryItemV1,65536,{pub owner:DomainId,pub inventory:String,pub requested_slot:u16,pub item:String,pub count:u32});
dto!(InventoryLiveOwnersV1,1,{pub owners:Vec<OwnerHandle>});
dto!(InitialInventoryItemsV1,1,{pub items:Vec<InitialInventoryItemV1>});
dto!(InventoryInitialTotalsV1,1,{pub items:Vec<ItemTotalV1>});
dto!(InventoryStackEndpointV1,1,{pub owner:OwnerHandle,pub inventory:String,pub stack_id:StackId});
dto!(InventorySlotEndpointV1,1,{pub owner:OwnerHandle,pub inventory:String,pub slot:u16});

#[derive(Clone, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum InventoryActionV1 {
    Move {
        source: InventoryStackEndpointV1,
        destination: InventorySlotEndpointV1,
    },
    Split {
        source: InventoryStackEndpointV1,
        destination: InventorySlotEndpointV1,
        amount: u32,
    },
    Merge {
        source: InventoryStackEndpointV1,
        destination: InventoryStackEndpointV1,
        amount: u32,
    },
    Consume {
        source: InventoryStackEndpointV1,
        amount: u32,
    },
    /// None explicitly appends one slot; Some uses an existing allocated slot.
    PlaceOverflow {
        source: InventoryStackEndpointV1,
        slot: Option<u16>,
    },
}
impl<'de> Deserialize<'de> for InventoryActionV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let _scope = InventoryDecodeScope::enter();
        // A derived internally tagged enum buffers arbitrary Content before
        // endpoint admission. Decode the named fields directly instead, and
        // check the variant's exact field set without buffering unknown data.
        struct Arg<T>(Option<T>);
        impl<T> Default for Arg<T> {
            fn default() -> Self {
                Self(None)
            }
        }
        impl<'de, T: Field> Deserialize<'de> for Arg<T> {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                T::field(d).map(|v| Self(Some(v)))
            }
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Destination {
            owner: OwnerHandle,
            #[serde(deserialize_with = "field")]
            inventory: String,
            #[serde(default)]
            slot: Arg<u16>,
            #[serde(default)]
            stack_id: Arg<StackId>,
        }
        impl Field for Destination {
            fn field<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                crate::json::object(d)
            }
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            #[serde(rename = "type", deserialize_with = "field")]
            kind: String,
            source: InventoryStackEndpointV1,
            #[serde(default)]
            destination: Arg<Destination>,
            #[serde(default)]
            amount: Arg<u32>,
            #[serde(default)]
            slot: Arg<Option<u16>>,
        }
        let f: Fields = crate::json::object(d)
            .map_err(|_| <D::Error as de::Error>::custom("inventory action invalid"))?;
        let invalid = || <D::Error as de::Error>::custom("inventory action invalid");
        let source = f.source;
        match f.kind.as_str() {
            "move" | "split" => {
                if f.slot.0.is_some() {
                    return Err(invalid());
                }
                let d = f.destination.0.ok_or_else(invalid)?;
                if d.stack_id.0.is_some() {
                    return Err(invalid());
                }
                let destination = InventorySlotEndpointV1 {
                    owner: d.owner,
                    inventory: d.inventory,
                    slot: d.slot.0.ok_or_else(invalid)?,
                };
                if f.kind == "move" {
                    if f.amount.0.is_some() {
                        return Err(invalid());
                    }
                    Ok(Self::Move {
                        source,
                        destination,
                    })
                } else {
                    Ok(Self::Split {
                        source,
                        destination,
                        amount: f.amount.0.ok_or_else(invalid)?,
                    })
                }
            }
            "merge" => {
                if f.slot.0.is_some() {
                    return Err(invalid());
                }
                let d = f.destination.0.ok_or_else(invalid)?;
                if d.slot.0.is_some() {
                    return Err(invalid());
                }
                let destination = InventoryStackEndpointV1 {
                    owner: d.owner,
                    inventory: d.inventory,
                    stack_id: d.stack_id.0.ok_or_else(invalid)?,
                };
                Ok(Self::Merge {
                    source,
                    destination,
                    amount: f.amount.0.ok_or_else(invalid)?,
                })
            }
            "consume" => {
                if f.destination.0.is_some() || f.slot.0.is_some() {
                    return Err(invalid());
                }
                Ok(Self::Consume {
                    source,
                    amount: f.amount.0.ok_or_else(invalid)?,
                })
            }
            "place_overflow" => {
                if f.destination.0.is_some() || f.amount.0.is_some() {
                    return Err(invalid());
                }
                Ok(Self::PlaceOverflow {
                    source,
                    slot: f.slot.0.ok_or_else(invalid)?,
                })
            }
            _ => Err(invalid()),
        }
    }
}
impl fmt::Debug for InventoryActionV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("InventoryActionV1 { .. }")
    }
}
impl Field for InventoryActionV1 {
    fn field<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::deserialize(d)
    }
}
impl Graph for InventoryActionV1 {
    const LIMIT: usize = 1;
    fn walk(&self, b: &mut Budget) -> Result<()> {
        match self {
            Self::Move {
                source,
                destination,
            }
            | Self::Split {
                source,
                destination,
                ..
            } => {
                source.walk(b)?;
                destination.walk(b)?;
            }
            Self::Merge {
                source,
                destination,
                ..
            } => {
                source.walk(b)?;
                destination.walk(b)?;
            }
            Self::Consume { source, .. } | Self::PlaceOverflow { source, .. } => source.walk(b)?,
        }
        Ok(())
    }
}
dto!(InventoryActionRequestV1,1,{pub expected_inventory_revision:u64,pub action:InventoryActionV1});
dto!(InventoryRestoreRequestV1,1,{pub expected_inventory_revision:u64,pub owners:Vec<OwnerInventoriesV1>});

impl InventoryStateSnapshotV1 {
    /// Allocation and encoding admission only; semantic state validation also
    /// requires the compiled contract, live owners and immutable initial totals.
    pub fn validate_bounds(&self) -> Result<()> {
        graph(self)?;
        encoded(self, MAX_INVENTORY_STATE_BYTES)?;
        Ok(())
    }
    /// Bounded JSON persistence DTO decoding only; callers must still validate
    /// conservation and owner incarnations before admission.
    pub fn parse(text: &str) -> Result<Self> {
        ensure!(
            text.len() <= MAX_INVENTORY_STATE_BYTES,
            "inventory state byte limit"
        );
        serde_json::from_str(text).map_err(|_| anyhow::anyhow!("inventory state JSON invalid"))
    }
}

impl InventoryContractsSnapshotV1 {
    pub fn validate_bounds(&self) -> Result<()> {
        graph(self)?;
        encoded(self, MAX_INVENTORY_CONTRACT_BYTES)?;
        Ok(())
    }
}

impl InventoryInitialTotalsV1 {
    pub fn validate_bounds(&self) -> Result<()> {
        graph(self)?;
        Ok(())
    }
}

impl InventoryRestoreRequestV1 {
    pub fn validate_bounds(&self) -> Result<()> {
        graph(self)?;
        encoded(self, MAX_INVENTORY_GRAPH_BYTES)?;
        Ok(())
    }
}

/// Conservative graph charge for a borrowed inventory collection, including
/// caller spare capacity and the capacity needed to decode its encoding.
/// Composing requests must sum these charges with their other owned fields.
pub fn inventory_collection_bytes(inventories: &Vec<InventoryStateV1>) -> Result<usize> {
    graph(inventories)
}

fn encoded(value: &impl Serialize, limit: usize) -> Result<usize> {
    struct Counter(usize, usize);
    impl Write for Counter {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0 = self
                .0
                .checked_add(b.len())
                .filter(|v| *v <= self.1)
                .ok_or_else(|| std::io::Error::other("inventory byte limit"))?;
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut c = Counter(0, limit);
    serde_json::to_writer(&mut c, value)
        .map_err(|_| anyhow::anyhow!("inventory encoding limit"))?;
    Ok(c.0)
}
fn digest(prefix: &[u8], value: &impl Serialize, limit: usize) -> Result<String> {
    encoded(value, limit)?;
    struct Hasher(Sha256);
    impl Write for Hasher {
        fn write(&mut self, b: &[u8]) -> std::io::Result<usize> {
            self.0.update(b);
            Ok(b.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    let mut h = Hasher(Sha256::new());
    h.0.update(prefix);
    serde_json::to_writer(&mut h, value)
        .map_err(|_| anyhow::anyhow!("inventory encoding invalid"))?;
    Ok(crate::hex_lower(&h.0.finalize()))
}
fn valid_digest(v: &str) -> bool {
    v.len() == 64
        && v.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}
fn add(total: &mut u64, count: u64) -> Result<()> {
    *total = total
        .checked_add(count)
        .ok_or_else(|| anyhow::anyhow!("inventory quantity overflow"))?;
    Ok(())
}
fn bounded_sum(total: &mut usize, n: usize, max: usize) -> Result<()> {
    *total = total
        .checked_add(n)
        .filter(|v| *v <= max)
        .ok_or_else(|| anyhow::anyhow!("inventory aggregate limit"))?;
    Ok(())
}

/// Private canonical, validated declarations and selected-map bindings.
#[derive(Clone)]
pub struct CompiledInventoryContractsV1 {
    snapshot: Arc<InventoryContractsSnapshotV1>,
    bindings: BTreeMap<DomainId, usize>,
}
impl fmt::Debug for CompiledInventoryContractsV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CompiledInventoryContractsV1")
            .field("owners", &self.bindings.len())
            .finish_non_exhaustive()
    }
}
impl CompiledInventoryContractsV1 {
    pub fn compile(
        mut definition: InventoryContractsV1,
        catalog: &CompiledNativeCatalog,
    ) -> Result<Self> {
        // The compiled snapshot adds its identity and enclosing object to the
        // definition. Check this before canonicalization or any expansion.
        let mut snapshot_bytes = Budget::default();
        snapshot_bytes.add(size_of::<InventoryContractsSnapshotV1>() + 64)?;
        definition.walk(&mut snapshot_bytes)?;
        encoded(&definition, MAX_INVENTORY_CONTRACT_BYTES)?;
        ensure!(
            definition.format == 1
                && definition.native_catalog_id == catalog.identity()
                && valid_digest(&definition.native_catalog_id),
            "inventory contract identity invalid"
        );
        let map = catalog
            .map(&definition.native_map)
            .ok_or_else(|| anyhow::anyhow!("inventory selected map missing"))?;
        ensure!(
            map.placements.len() <= MAX_INVENTORY_OWNERS,
            "inventory owner limit"
        );
        let mut items = BTreeSet::new();
        for item in &definition.items {
            ensure!(
                items.insert(item.key.as_str())
                    && item.stack_limit > 0
                    && item.stack_limit <= MAX_INVENTORY_STACK_COUNT,
                "inventory item definition invalid"
            );
        }
        let used: BTreeSet<_> = map.placements.iter().map(|p| p.class.as_str()).collect();
        let mut classes = BTreeMap::new();
        for c in &definition.classes {
            ensure!(
                used.contains(c.native_class.as_str())
                    && classes.insert(c.native_class.as_str(), c).is_none(),
                "inventory class coverage invalid"
            );
            let mut inventories = BTreeSet::new();
            for inv in &c.inventories {
                ensure!(
                    inventories.insert(inv.key.as_str())
                        && usize::from(inv.initial_slots) <= inv.slot_rules.len()
                        && inv.overflow_limit <= 256,
                    "inventory recipe invalid"
                );
                for rule in &inv.slot_rules {
                    let mut eligibility = BTreeSet::new();
                    for item in &rule.allowed_items {
                        ensure!(
                            items.contains(item.as_str()) && eligibility.insert(item.as_str()),
                            "inventory eligibility invalid"
                        );
                    }
                }
            }
        }
        ensure!(
            classes.len() == used.len(),
            "inventory class coverage incomplete"
        );
        // Bound recipe expansion before any owner inventory is materialized.
        let mut instances = 0;
        let mut bytes = Budget::default();
        bytes.add(size_of::<InventoryStateSnapshotV1>() + 64)?;
        bytes.vector::<ItemTotalV1>(definition.items.len(), definition.items.len())?;
        for item in &definition.items {
            bytes.add(item.key.len())?;
        }
        bytes.vector::<OwnerInventoriesV1>(map.placements.len(), map.placements.len())?;
        for p in &map.placements {
            let class = classes[p.class.as_str()];
            bounded_sum(
                &mut instances,
                class.inventories.len(),
                MAX_INVENTORY_INSTANCES,
            )?;
            bytes.vector::<InventoryStateV1>(class.inventories.len(), class.inventories.len())?;
            for inv in &class.inventories {
                bytes.add(inv.key.len())?;
                let slots = usize::from(inv.initial_slots);
                bytes.vector::<Option<StackV1>>(slots, slots)?;
            }
        }
        drop(classes);
        drop(items);
        for c in &mut definition.classes {
            c.inventories.sort_by(|a, b| a.key.cmp(&b.key));
            for inv in &mut c.inventories {
                for rule in &mut inv.slot_rules {
                    rule.allowed_items.sort();
                }
            }
        }
        definition
            .classes
            .sort_by(|a, b| a.native_class.cmp(&b.native_class));
        definition.items.sort_by(|a, b| a.key.cmp(&b.key));
        let identity = digest(
            b"authored-inventory-contract-v1\0",
            &definition,
            MAX_INVENTORY_CONTRACT_BYTES,
        )?;
        let bindings = map
            .placements
            .iter()
            .enumerate()
            .map(|(i, p)| {
                let class = definition
                    .classes
                    .binary_search_by(|c| c.native_class.cmp(&p.class))
                    .expect("validated class");
                (DomainId(i as u64 + 1), class)
            })
            .collect();
        let snapshot = Arc::new(InventoryContractsSnapshotV1 {
            contract_id: identity,
            definition,
        });
        graph(snapshot.as_ref())?;
        encoded(snapshot.as_ref(), MAX_INVENTORY_CONTRACT_BYTES)?;
        Ok(Self { snapshot, bindings })
    }
    /// Bounded JSON or TOML declarations through the same strict compiler.
    pub fn parse(text: &str, catalog: &CompiledNativeCatalog) -> Result<Self> {
        ensure!(
            text.len() <= MAX_INVENTORY_CONTRACT_BYTES,
            "inventory contract byte limit"
        );
        // Select once; malformed JSON cannot be reinterpreted as TOML. Both
        // formats use the same bounded fields and strict named-object DTOs.
        let d = if text.trim_start().starts_with('{') {
            serde_json::from_str(text)
                .map_err(|_| anyhow::anyhow!("inventory contract JSON invalid"))?
        } else {
            toml::from_str(text).map_err(|_| anyhow::anyhow!("inventory contract TOML invalid"))?
        };
        Self::compile(d, catalog)
    }
    pub fn load(path: &Path, catalog: &CompiledNativeCatalog) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(crate::nonblocking_flag())
            .open(path)
            .map_err(|_| anyhow::anyhow!("inventory contract open failed"))?;
        let meta = file
            .metadata()
            .map_err(|_| anyhow::anyhow!("inventory contract metadata failed"))?;
        ensure!(
            meta.is_file() && meta.len() <= MAX_INVENTORY_CONTRACT_BYTES as u64,
            "inventory contract regular file or size invalid"
        );
        let mut text = String::new();
        file.take(MAX_INVENTORY_CONTRACT_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .map_err(|_| anyhow::anyhow!("inventory contract read failed"))?;
        Self::parse(&text, catalog)
    }
    pub fn identity(&self) -> &str {
        &self.snapshot.contract_id
    }
    pub fn definition(&self) -> &InventoryContractsV1 {
        &self.snapshot.definition
    }
    pub fn snapshot(&self) -> Arc<InventoryContractsSnapshotV1> {
        Arc::clone(&self.snapshot)
    }
    pub fn from_snapshot(
        snapshot: &InventoryContractsSnapshotV1,
        catalog: &CompiledNativeCatalog,
    ) -> Result<Self> {
        graph(snapshot)?;
        encoded(snapshot, MAX_INVENTORY_CONTRACT_BYTES)?;
        let result = Self::compile(snapshot.definition.clone(), catalog)?;
        ensure!(
            snapshot.contract_id == result.identity(),
            "inventory contract snapshot identity mismatch"
        );
        Ok(result)
    }
    fn recipes(&self, id: DomainId) -> Result<&[InventoryDefinitionV1]> {
        let i = self
            .bindings
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("inventory owner missing"))?;
        Ok(&self.definition().classes[*i].inventories)
    }
    fn recipe(&self, id: DomainId, key: &str) -> Result<&InventoryDefinitionV1> {
        self.recipes(id)?
            .iter()
            .find(|r| r.key == key)
            .ok_or_else(|| anyhow::anyhow!("inventory instance missing"))
    }
    fn limit(&self, item: &str) -> Result<u32> {
        self.definition()
            .items
            .iter()
            .find(|i| i.key == item)
            .map(|i| i.stack_limit)
            .ok_or_else(|| anyhow::anyhow!("inventory item undeclared"))
    }
    fn live(&self, live: &InventoryLiveOwnersV1) -> Result<BTreeMap<DomainId, OwnerHandle>> {
        graph(live)?;
        ensure!(
            live.owners.len() == self.bindings.len(),
            "inventory live owner coverage invalid"
        );
        let mut result = BTreeMap::new();
        for h in &live.owners {
            ensure!(
                h.incarnation > 0
                    && self.bindings.contains_key(&h.id)
                    && result.insert(h.id, *h).is_none(),
                "inventory live owner invalid"
            );
        }
        Ok(result)
    }
}

fn totals<'a>(
    c: &CompiledInventoryContractsV1,
    t: &'a InventoryInitialTotalsV1,
) -> Result<BTreeMap<&'a str, u64>> {
    ensure!(
        t.items.len() == c.definition().items.len(),
        "inventory totals coverage invalid"
    );
    let mut values = BTreeMap::new();
    for v in &t.items {
        c.limit(&v.item)?;
        ensure!(
            values.insert(v.item.as_str(), v.count).is_none(),
            "inventory duplicate item total"
        );
    }
    Ok(values)
}
fn eligible(recipe: &InventoryDefinitionV1, slot: usize, item: &str) -> bool {
    recipe
        .slot_rules
        .get(slot)
        .is_some_and(|r| r.allowed_items.iter().any(|v| v == item))
}
fn input_budget(values: &[&dyn InventoryGraph]) -> Result<usize> {
    let mut b = Budget::default();
    for v in values {
        v.inventory_walk(&mut b)?;
    }
    Ok(b.0)
}
// Object-safe adapter keeps each operation's simultaneous borrowed graph under
// one allowance, including all nested spare capacities, before cloning.
trait InventoryGraph {
    fn inventory_walk(&self, b: &mut Budget) -> Result<()>;
}
impl<T: Graph> InventoryGraph for T {
    fn inventory_walk(&self, b: &mut Budget) -> Result<()> {
        b.add(size_of::<T>())?;
        self.walk(b)
    }
}

/// Full admission, including unrelated owners and stacks. Callers must bind
/// initial totals to provenance separately; conservation alone proves no history.
pub fn validate_inventory_state(
    contracts: &CompiledInventoryContractsV1,
    state: &InventoryStateSnapshotV1,
    initial_totals: &InventoryInitialTotalsV1,
    live: &InventoryLiveOwnersV1,
) -> Result<()> {
    input_budget(&[state, initial_totals, live])?;
    encoded(state, MAX_INVENTORY_STATE_BYTES)?;
    ensure!(
        state.format == 1
            && state.contract_id == contracts.identity()
            && state.revision >= 1
            && state.next_stack_id >= 1,
        "inventory state header invalid"
    );
    let handles = contracts.live(live)?;
    let initial = totals(contracts, initial_totals)?;
    ensure!(
        state.consumed.len() == initial.len(),
        "inventory consumed coverage invalid"
    );
    let mut quantities = BTreeMap::new();
    for v in &state.consumed {
        ensure!(
            initial.contains_key(v.item.as_str())
                && quantities.insert(v.item.as_str(), v.count).is_none(),
            "inventory consumed total invalid"
        );
    }
    ensure!(
        state.owners.len() == handles.len(),
        "inventory state owner coverage invalid"
    );
    let mut owners = BTreeSet::new();
    let mut ids = BTreeSet::new();
    let mut instances = 0;
    let mut stacks = 0;
    for owner in &state.owners {
        ensure!(
            handles.get(&owner.owner.id) == Some(&owner.owner) && owners.insert(owner.owner.id),
            "inventory state owner invalid"
        );
        let recipes = contracts.recipes(owner.owner.id)?;
        ensure!(
            owner.inventories.len() == recipes.len(),
            "inventory state instance coverage invalid"
        );
        bounded_sum(
            &mut instances,
            owner.inventories.len(),
            MAX_INVENTORY_INSTANCES,
        )?;
        let mut inventories = BTreeSet::new();
        for inv in &owner.inventories {
            ensure!(
                inventories.insert(inv.key.as_str()),
                "inventory duplicate instance"
            );
            let recipe = contracts.recipe(owner.owner.id, &inv.key)?;
            ensure!(
                inv.slots.len() >= usize::from(recipe.initial_slots)
                    && inv.slots.len() <= recipe.slot_rules.len()
                    && inv.overflow.len() <= usize::from(recipe.overflow_limit),
                "inventory allocation or overflow invalid"
            );
            let mut validate_stack = |s: &StackV1| -> Result<()> {
                ensure!(
                    s.id.0 > 0
                        && s.id.0 < state.next_stack_id
                        && ids.insert(s.id)
                        && s.count > 0
                        && s.count <= contracts.limit(&s.item)?,
                    "inventory stack invalid"
                );
                bounded_sum(&mut stacks, 1, MAX_INVENTORY_STACKS)?;
                let q = quantities
                    .get_mut(s.item.as_str())
                    .ok_or_else(|| anyhow::anyhow!("inventory quantity item missing"))?;
                add(q, u64::from(s.count))
            };
            for (slot, stack) in inv.slots.iter().enumerate() {
                if let Some(s) = stack {
                    ensure!(
                        eligible(recipe, slot, &s.item),
                        "inventory slot eligibility invalid"
                    );
                    validate_stack(s)?;
                }
            }
            for d in &inv.overflow {
                ensure!(
                    d.requested_slot <= 255,
                    "inventory overflow location invalid"
                );
                validate_stack(&d.stack)?;
            }
        }
    }
    ensure!(quantities == initial, "inventory conservation mismatch");
    Ok(())
}

fn canonical_state(state: &mut InventoryStateSnapshotV1) {
    state.owners.sort_by_key(|o| o.owner.id);
    for owner in &mut state.owners {
        owner.inventories.sort_by(|a, b| a.key.cmp(&b.key));
    }
    state.consumed.sort_by(|a, b| a.item.cmp(&b.item));
}
#[derive(Clone)]
pub struct PreparedInitialInventoryV1 {
    state: InventoryStateSnapshotV1,
    initial_totals: InventoryInitialTotalsV1,
    initial_state_digest: String,
}
impl fmt::Debug for PreparedInitialInventoryV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedInitialInventoryV1")
            .field("owners", &self.state.owners.len())
            .finish_non_exhaustive()
    }
}
impl PreparedInitialInventoryV1 {
    pub fn state(&self) -> &InventoryStateSnapshotV1 {
        &self.state
    }
    pub fn initial_totals(&self) -> &InventoryInitialTotalsV1 {
        &self.initial_totals
    }
    pub fn initial_state_digest(&self) -> &str {
        &self.initial_state_digest
    }
    pub fn into_parts(self) -> (InventoryStateSnapshotV1, InventoryInitialTotalsV1, String) {
        (self.state, self.initial_totals, self.initial_state_digest)
    }
}

pub fn prepare_initial_inventory(
    contracts: &CompiledInventoryContractsV1,
    live: &InventoryLiveOwnersV1,
    items: &InitialInventoryItemsV1,
) -> Result<PreparedInitialInventoryV1> {
    input_budget(&[live, items])?;
    encoded(items, MAX_INVENTORY_GRAPH_BYTES)?;
    let handles = contracts.live(live)?;
    let mut counts = BTreeMap::new();
    let mut initial: BTreeMap<&str, u64> = contracts
        .definition()
        .items
        .iter()
        .map(|i| (i.key.as_str(), 0))
        .collect();
    let mut retained = Budget::default();
    retained.add(size_of::<PreparedInitialInventoryV1>() + 128)?;
    // Final validation retains the supplied live set alongside the result.
    // Include its caller spare and reload capacity before expanding recipes.
    live.inventory_walk(&mut retained)?;
    retained.vector::<OwnerInventoriesV1>(handles.len(), handles.len())?;
    retained.vector::<ItemTotalV1>(initial.len(), initial.len())?;
    retained.vector::<ItemTotalV1>(initial.len(), initial.len())?;
    retained.add(initial.keys().map(|s| s.len() * 2).sum())?;
    for id in handles.keys() {
        let inventories = contracts.recipes(*id)?.len();
        retained.vector::<InventoryStateV1>(inventories, inventories)?;
        for recipe in contracts.recipes(*id)? {
            retained.add(recipe.key.len())?;
            // Empty vectors grow to four when the zero-slot route allocates.
            let slots = if recipe.initial_slots == 0 && recipe.force_add_when_zero {
                4
            } else {
                usize::from(recipe.initial_slots)
            };
            retained.vector::<Option<StackV1>>(slots, slots)?;
        }
    }
    // Validate the complete source graph and aggregate occurrences before any
    // item/key clone or expanded state allocation. IDs follow this input order.
    for item in &items.items {
        ensure!(
            handles.contains_key(&item.owner)
                && item.requested_slot <= 255
                && item.count > 0
                && item.count <= contracts.limit(&item.item)?,
            "inventory initial item invalid"
        );
        contracts.recipe(item.owner, &item.inventory)?;
        let count = counts
            .entry((item.owner, item.inventory.as_str()))
            .or_insert(0_usize);
        bounded_sum(count, 1, 512)?;
        add(
            initial
                .get_mut(item.item.as_str())
                .ok_or_else(|| anyhow::anyhow!("inventory initial item missing"))?,
            u64::from(item.count),
        )?;
        // Each occurrence owns its item string, in either slots or overflow.
        retained.add(item.item.len())?;
    }
    for ((owner, inventory), count) in &counts {
        // Before routing, any occurrence may overflow, up to the declared cap.
        // Native push and reload both grow these nonempty vectors from four
        // by doubling; charge the shared policy, including the last threshold.
        let overflow = (*count).min(usize::from(
            contracts.recipe(*owner, inventory)?.overflow_limit,
        ));
        retained.vector::<DeferredStackV1>(overflow, 0)?;
    }
    let mut state = InventoryStateSnapshotV1 {
        format: 1,
        contract_id: contracts.identity().to_owned(),
        revision: 1,
        next_stack_id: u64::try_from(items.items.len())
            .ok()
            .and_then(|n| n.checked_add(1))
            .ok_or_else(|| anyhow::anyhow!("inventory allocator exhausted"))?,
        consumed: initial
            .keys()
            .map(|k| ItemTotalV1 {
                item: (*k).to_owned(),
                count: 0,
            })
            .collect(),
        owners: handles
            .values()
            .map(|owner| OwnerInventoriesV1 {
                owner: *owner,
                inventories: contracts
                    .recipes(owner.id)
                    .expect("validated owner")
                    .iter()
                    .map(|r| InventoryStateV1 {
                        key: r.key.clone(),
                        slots: vec![None; usize::from(r.initial_slots)],
                        overflow: Vec::new(),
                    })
                    .collect(),
            })
            .collect(),
    };
    for (index, item) in items.items.iter().enumerate() {
        let recipe = contracts.recipe(item.owner, &item.inventory)?;
        let owner = &mut state.owners[(item.owner.0 - 1) as usize];
        let inventory_index = owner
            .inventories
            .binary_search_by(|i| i.key.cmp(&item.inventory))
            .expect("validated inventory");
        let inv = &mut owner.inventories[inventory_index];
        let slot = usize::from(item.requested_slot);
        let accepted =
            inv.slots.get(slot).is_some_and(Option::is_none) && eligible(recipe, slot, &item.item);
        let stack = StackV1 {
            id: StackId(index as u64 + 1),
            item: item.item.clone(),
            count: item.count,
        };
        match resolve_inventory_slot_restore(accepted, inv.slots.len()) {
            InventorySlotOverflowDisposition::PlacedInSlot => inv.slots[slot] = Some(stack),
            InventorySlotOverflowDisposition::ForceAddItemsWhenEmpty => {
                ensure!(
                    recipe.force_add_when_zero && eligible(recipe, 0, &item.item),
                    "inventory zero-slot route refused"
                );
                inv.slots.push(Some(stack));
            }
            InventorySlotOverflowDisposition::AppendToDeferredOverflow => {
                ensure!(
                    inv.overflow.len() < usize::from(recipe.overflow_limit),
                    "inventory overflow saturated"
                );
                inv.overflow.push(DeferredStackV1 {
                    stack,
                    requested_slot: item.requested_slot,
                });
            }
        }
    }
    canonical_state(&mut state);
    let initial_totals = InventoryInitialTotalsV1 {
        items: initial
            .into_iter()
            .map(|(item, count)| ItemTotalV1 {
                item: item.to_owned(),
                count,
            })
            .collect(),
    };
    validate_inventory_state(contracts, &state, &initial_totals, live)?;
    let initial_state_digest = digest(
        b"authored-inventory-initial-state-v1\0",
        &state,
        MAX_INVENTORY_STATE_BYTES,
    )?;
    Ok(PreparedInitialInventoryV1 {
        state,
        initial_totals,
        initial_state_digest,
    })
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum InventoryActionOutcomeV1 {
    Applied,
    NoChange,
}
impl<'de> Deserialize<'de> for InventoryActionOutcomeV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let _scope = InventoryDecodeScope::enter();
        let value = String::field(d)
            .map_err(|_| <D::Error as de::Error>::custom("inventory outcome invalid"))?;
        match value.as_str() {
            "applied" => Ok(Self::Applied),
            "no_change" => Ok(Self::NoChange),
            _ => Err(de::Error::custom("inventory outcome invalid")),
        }
    }
}
pub struct PreparedInventoryStateV1 {
    state: InventoryStateSnapshotV1,
    outcome: InventoryActionOutcomeV1,
}
impl fmt::Debug for PreparedInventoryStateV1 {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PreparedInventoryStateV1")
            .field("owners", &self.state.owners.len())
            .finish_non_exhaustive()
    }
}
impl PreparedInventoryStateV1 {
    pub fn state(&self) -> &InventoryStateSnapshotV1 {
        &self.state
    }
    pub fn outcome(&self) -> InventoryActionOutcomeV1 {
        self.outcome
    }
    pub fn into_state(self) -> InventoryStateSnapshotV1 {
        self.state
    }
}
#[derive(Clone, Copy, PartialEq, Eq)]
enum Location {
    Slot(usize),
    Overflow(usize),
}
#[derive(Clone, Copy, PartialEq, Eq)]
struct Address {
    owner: usize,
    inventory: usize,
    location: Location,
}
fn inventory_address(
    state: &InventoryStateSnapshotV1,
    owner: OwnerHandle,
    key: &str,
) -> Result<(usize, usize)> {
    let o = state
        .owners
        .iter()
        .position(|o| o.owner == owner)
        .ok_or_else(|| anyhow::anyhow!("inventory endpoint owner invalid"))?;
    let i = state.owners[o]
        .inventories
        .iter()
        .position(|i| i.key == key)
        .ok_or_else(|| anyhow::anyhow!("inventory endpoint instance invalid"))?;
    Ok((o, i))
}
fn stack_address(
    state: &InventoryStateSnapshotV1,
    e: &InventoryStackEndpointV1,
) -> Result<Address> {
    let (owner, inventory) = inventory_address(state, e.owner, &e.inventory)?;
    let inv = &state.owners[owner].inventories[inventory];
    let location = if let Some(i) = inv
        .slots
        .iter()
        .position(|s| s.as_ref().is_some_and(|s| s.id == e.stack_id))
    {
        Location::Slot(i)
    } else if let Some(i) = inv.overflow.iter().position(|d| d.stack.id == e.stack_id) {
        Location::Overflow(i)
    } else {
        anyhow::bail!("inventory endpoint stack missing");
    };
    Ok(Address {
        owner,
        inventory,
        location,
    })
}
fn at(state: &InventoryStateSnapshotV1, a: Address) -> &StackV1 {
    let inv = &state.owners[a.owner].inventories[a.inventory];
    match a.location {
        Location::Slot(i) => inv.slots[i].as_ref().expect("validated stack"),
        Location::Overflow(i) => &inv.overflow[i].stack,
    }
}
fn at_mut(state: &mut InventoryStateSnapshotV1, a: Address) -> &mut StackV1 {
    let inv = &mut state.owners[a.owner].inventories[a.inventory];
    match a.location {
        Location::Slot(i) => inv.slots[i].as_mut().expect("validated stack"),
        Location::Overflow(i) => &mut inv.overflow[i].stack,
    }
}
fn remove(state: &mut InventoryStateSnapshotV1, a: Address) -> StackV1 {
    let inv = &mut state.owners[a.owner].inventories[a.inventory];
    match a.location {
        Location::Slot(i) => inv.slots[i].take().expect("validated stack"),
        Location::Overflow(i) => inv.overflow.remove(i).stack,
    }
}
fn destination(
    c: &CompiledInventoryContractsV1,
    state: &InventoryStateSnapshotV1,
    e: &InventorySlotEndpointV1,
    item: &str,
) -> Result<Address> {
    let (owner, inventory) = inventory_address(state, e.owner, &e.inventory)?;
    let slot = usize::from(e.slot);
    ensure!(
        state.owners[owner].inventories[inventory]
            .slots
            .get(slot)
            .is_some_and(Option::is_none)
            && eligible(c.recipe(e.owner.id, &e.inventory)?, slot, item),
        "inventory destination refused"
    );
    Ok(Address {
        owner,
        inventory,
        location: Location::Slot(slot),
    })
}
fn install(state: &mut InventoryStateSnapshotV1, a: Address, stack: StackV1) {
    let Location::Slot(slot) = a.location else {
        unreachable!("slot destination")
    };
    state.owners[a.owner].inventories[a.inventory].slots[slot] = Some(stack);
}
fn revision(state: &InventoryStateSnapshotV1, expected: u64) -> Result<u64> {
    ensure!(state.revision == expected, "inventory revision conflict");
    state
        .revision
        .checked_add(1)
        .ok_or_else(|| anyhow::anyhow!("inventory revision exhausted"))
}

/// Whole-state validation precedes CAS and action inspection. Even an exact
/// self-slot Move validates unrelated contents and the expected revision. It
/// returns NoChange with the same revision and needs no new revision value.
pub fn prepare_inventory_action(
    contracts: &CompiledInventoryContractsV1,
    current: &InventoryStateSnapshotV1,
    initial_totals: &InventoryInitialTotalsV1,
    live: &InventoryLiveOwnersV1,
    request: &InventoryActionRequestV1,
) -> Result<PreparedInventoryStateV1> {
    let admitted_bytes = input_budget(&[current, initial_totals, live, request])?;
    validate_inventory_state(contracts, current, initial_totals, live)?;
    ensure!(
        current.revision == request.expected_inventory_revision,
        "inventory revision conflict"
    );
    enum Plan {
        Noop,
        Move(Address, Address),
        Split(Address, Address, u32, u64),
        Merge(Address, Address, u32),
        Consume(Address, u32),
        Place(Address, usize, bool),
    }
    let plan = match &request.action {
        InventoryActionV1::Move {
            source,
            destination: dest,
        } => {
            let s = stack_address(current, source)?;
            let (o, i) = inventory_address(current, dest.owner, &dest.inventory)?;
            if s.owner == o
                && s.inventory == i
                && s.location == Location::Slot(usize::from(dest.slot))
            {
                Plan::Noop
            } else {
                Plan::Move(
                    s,
                    destination(contracts, current, dest, &at(current, s).item)?,
                )
            }
        }
        InventoryActionV1::Split {
            source,
            destination: dest,
            amount,
        } => {
            let s = stack_address(current, source)?;
            let stack = at(current, s);
            ensure!(
                *amount > 0 && *amount < stack.count,
                "inventory split amount invalid"
            );
            let d = destination(contracts, current, dest, &stack.item)?;
            let next = current
                .next_stack_id
                .checked_add(1)
                .ok_or_else(|| anyhow::anyhow!("inventory allocator exhausted"))?;
            Plan::Split(s, d, *amount, next)
        }
        InventoryActionV1::Merge {
            source,
            destination: dest,
            amount,
        } => {
            let s = stack_address(current, source)?;
            let d = stack_address(current, dest)?;
            let a = at(current, s);
            let b = at(current, d);
            ensure!(
                s != d
                    && a.item == b.item
                    && *amount > 0
                    && *amount <= a.count
                    && b.count
                        .checked_add(*amount)
                        .is_some_and(|n| n <= contracts.limit(&b.item).unwrap_or(0)),
                "inventory merge refused"
            );
            Plan::Merge(s, d, *amount)
        }
        InventoryActionV1::Consume { source, amount } => {
            let s = stack_address(current, source)?;
            ensure!(
                *amount > 0 && *amount <= at(current, s).count,
                "inventory consume amount invalid"
            );
            let value = current
                .consumed
                .iter()
                .find(|v| v.item == at(current, s).item)
                .expect("validated total");
            ensure!(
                value.count.checked_add(u64::from(*amount)).is_some(),
                "inventory consumed quantity overflow"
            );
            Plan::Consume(s, *amount)
        }
        InventoryActionV1::PlaceOverflow { source, slot } => {
            let s = stack_address(current, source)?;
            ensure!(
                matches!(s.location, Location::Overflow(_)),
                "inventory overflow endpoint required"
            );
            let inv = &current.owners[s.owner].inventories[s.inventory];
            let index = slot.map_or(inv.slots.len(), usize::from);
            ensure!(
                (slot.is_none() || inv.slots.get(index).is_some_and(Option::is_none))
                    && eligible(
                        contracts.recipe(source.owner.id, &source.inventory)?,
                        index,
                        &at(current, s).item
                    ),
                "inventory overflow placement refused"
            );
            Plan::Place(s, index, slot.is_none())
        }
    };
    let next_revision = if matches!(plan, Plan::Noop) {
        current.revision
    } else {
        revision(current, request.expected_inventory_revision)?
    };
    let extra = match plan {
        Plan::Split(..) => {
            let active: usize = current
                .owners
                .iter()
                .flat_map(|o| &o.inventories)
                .map(|i| i.slots.iter().flatten().count() + i.overflow.len())
                .sum();
            ensure!(
                active < MAX_INVENTORY_STACKS,
                "inventory active stack limit"
            );
            64
        }
        Plan::Place(s, _, true) => {
            let slots = &current.owners[s.owner].inventories[s.inventory].slots;
            let before = slots
                .capacity()
                .max(decode_capacity::<Option<StackV1>>(slots.len())?);
            let after = slots
                .capacity()
                .max(decode_capacity::<Option<StackV1>>(slots.len() + 1)?);
            (after - before)
                .checked_mul(size_of::<Option<StackV1>>())
                .ok_or_else(|| anyhow::anyhow!("inventory allocation limit"))?
        }
        _ => 0,
    };
    ensure!(
        admitted_bytes
            .checked_add(extra)
            .is_some_and(|n| n <= MAX_INVENTORY_GRAPH_BYTES),
        "inventory result allocation limit"
    );
    let mut state = current.clone();
    let outcome = if matches!(plan, Plan::Noop) {
        InventoryActionOutcomeV1::NoChange
    } else {
        InventoryActionOutcomeV1::Applied
    };
    match plan {
        Plan::Noop => {}
        Plan::Move(s, d) => {
            let stack = remove(&mut state, s);
            install(&mut state, d, stack);
        }
        Plan::Split(s, d, amount, next) => {
            let stack = StackV1 {
                id: StackId(current.next_stack_id),
                item: at(current, s).item.clone(),
                count: amount,
            };
            at_mut(&mut state, s).count -= amount;
            install(&mut state, d, stack);
            state.next_stack_id = next;
        }
        Plan::Merge(s, d, amount) => {
            // Increase before removing: overflow removal can shift destination.
            at_mut(&mut state, d).count += amount;
            if at(&state, s).count == amount {
                remove(&mut state, s);
            } else {
                at_mut(&mut state, s).count -= amount;
            }
        }
        Plan::Consume(s, amount) => {
            let item = &at(current, s).item;
            state
                .consumed
                .iter_mut()
                .find(|v| v.item == *item)
                .expect("validated total")
                .count += u64::from(amount);
            if at(&state, s).count == amount {
                remove(&mut state, s);
            } else {
                at_mut(&mut state, s).count -= amount;
            }
        }
        Plan::Place(s, index, append) => {
            let stack = remove(&mut state, s);
            let inv = &mut state.owners[s.owner].inventories[s.inventory];
            if append {
                // The result preflight already covered this reload capacity.
                // Reserve explicitly so native push cannot grow beyond it.
                let capacity = decode_capacity::<Option<StackV1>>(inv.slots.len() + 1)?;
                if inv.slots.capacity() < capacity {
                    inv.slots
                        .try_reserve_exact(capacity - inv.slots.len())
                        .map_err(|_| anyhow::anyhow!("inventory allocation limit"))?;
                }
                inv.slots.push(Some(stack));
            } else {
                inv.slots[index] = Some(stack);
            }
        }
    }
    state.revision = next_revision;
    validate_inventory_state(contracts, &state, initial_totals, live)?;
    Ok(PreparedInventoryStateV1 { state, outcome })
}

/// Pure composition after lifecycle staging. Does not evaluate owner policies.
/// Every target supplies its complete inventories, and targeted current stack
/// triples are conserved across the entire target set, including overflow.
pub fn prepare_inventory_restore(
    contracts: &CompiledInventoryContractsV1,
    current: &InventoryStateSnapshotV1,
    initial_totals: &InventoryInitialTotalsV1,
    pre_live: &InventoryLiveOwnersV1,
    post_live: &InventoryLiveOwnersV1,
    request: &InventoryRestoreRequestV1,
) -> Result<PreparedInventoryStateV1> {
    input_budget(&[current, initial_totals, pre_live, post_live, request])?;
    encoded(request, MAX_INVENTORY_GRAPH_BYTES)?;
    validate_inventory_state(contracts, current, initial_totals, pre_live)?;
    let after = contracts.live(post_live)?;
    let next_revision = revision(current, request.expected_inventory_revision)?;
    ensure!(
        !request.owners.is_empty(),
        "inventory restore targets empty"
    );
    let mut targets = BTreeSet::new();
    for owner in &request.owners {
        ensure!(
            targets.insert(owner.owner.id) && after.get(&owner.owner.id) == Some(&owner.owner),
            "inventory restore target invalid"
        );
    }
    for owner in &current.owners {
        ensure!(
            targets.contains(&owner.owner.id) || after.get(&owner.owner.id) == Some(&owner.owner),
            "inventory non-target handle changed"
        );
    }
    fn triples<'a>(
        owners: impl Iterator<Item = &'a OwnerInventoriesV1>,
    ) -> Result<BTreeMap<StackId, (&'a str, u32)>> {
        let mut result = BTreeMap::new();
        let mut count = 0;
        let mut instances = 0;
        for o in owners {
            bounded_sum(&mut instances, o.inventories.len(), MAX_INVENTORY_INSTANCES)?;
            for i in &o.inventories {
                for s in i
                    .slots
                    .iter()
                    .flatten()
                    .chain(i.overflow.iter().map(|d| &d.stack))
                {
                    bounded_sum(&mut count, 1, MAX_INVENTORY_STACKS)?;
                    ensure!(
                        result.insert(s.id, (s.item.as_str(), s.count)).is_none(),
                        "inventory restore duplicate stack"
                    );
                }
            }
        }
        Ok(result)
    }
    ensure!(
        triples(
            current
                .owners
                .iter()
                .filter(|o| targets.contains(&o.owner.id))
        )? == triples(request.owners.iter())?,
        "inventory restore stack multiset mismatch"
    );
    // Bound both graphs before any clone, also covering the final retained graph.
    let mut retained = Budget::default();
    retained.add(size_of::<InventoryStateSnapshotV1>())?;
    current.walk(&mut retained)?;
    request.walk(&mut retained)?;
    let mut state = current.clone();
    for replacement in &request.owners {
        let owner = state
            .owners
            .iter_mut()
            .find(|o| o.owner.id == replacement.owner.id)
            .expect("validated target");
        *owner = replacement.clone();
    }
    state.revision = next_revision;
    validate_inventory_state(contracts, &state, initial_totals, post_live)?;
    Ok(PreparedInventoryStateV1 {
        state,
        outcome: InventoryActionOutcomeV1::Applied,
    })
}
