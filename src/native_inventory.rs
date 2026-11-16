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
    static DECODE_INITIAL_ITEMS: Cell<usize> = const { Cell::new(0) };
    static DECODE_INSTANCES: Cell<usize> = const { Cell::new(0) };
}
pub(crate) struct InventoryDecodeScope;
impl InventoryDecodeScope {
    pub(crate) fn charge_bytes(bytes: usize) -> Result<()> {
        charge::<serde::de::value::Error>(bytes)?;
        Ok(())
    }
    pub(crate) fn enter() -> Self {
        DECODE_DEPTH.with(|d| {
            if d.get() == 0 {
                DECODE_BYTES.with(|b| b.set(0));
                DECODE_STACKS.with(|b| b.set(0));
                DECODE_INITIAL_ITEMS.with(|b| b.set(0));
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
        "StackV1" => DECODE_STACKS.with(|c| increment(c, MAX_INVENTORY_STACKS)),
        "InitialInventoryItemV1" => {
            DECODE_INITIAL_ITEMS.with(|c| increment(c, MAX_INVENTORY_STACKS))
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
        "InventoryStateSnapshotV1"
        | "PlayerInventorySnapshotV1"
        | "PlayerInventorySnapshotV2"
        | "PlayerInventorySnapshot" => MAX_INVENTORY_STATE_BYTES,
        "PlayerInventoryRecipeV1" | "PlayerInventoryRecipeV2" | "PlayerInventoryRecipe" => {
            MAX_INVENTORY_CONTRACT_BYTES
        }
        "InitialInventoryItemsV1"
        | "InventoryRestoreRequestV1"
        | "AuthoredInventoryAuthorityV1" => MAX_INVENTORY_GRAPH_BYTES,
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
// Separate durable namespace: these handles never name spatial lifecycle owners.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct PlayerInventoryOwnerV1 {
    pub player_id: u64,
    pub incarnation: u64,
}
impl<'de> Deserialize<'de> for PlayerInventoryOwnerV1 {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            player_id: u64,
            incarnation: u64,
        }
        let fields: Fields = crate::json::object(d)
            .map_err(|_| <D::Error as de::Error>::custom("player inventory owner invalid"))?;
        Ok(Self {
            player_id: fields.player_id,
            incarnation: fields.incarnation,
        })
    }
}
leaf!(PlayerInventoryOwnerV1, 4096);
dto!(PlayerStarterItemV1,65536,{pub inventory:String,pub requested_slot:u16,pub item:String,pub count:u32});
dto!(PlayerInventoryRecipeV1,1,{pub format:u32,pub items:Vec<ItemDefinitionV1>,pub inventories:Vec<InventoryDefinitionV1>,pub starter:Vec<PlayerStarterItemV1>});
dto!(PlayerConsumableV2,1024,{pub item:String,pub oxygen:u32,pub water:u32,pub food:u32});
dto!(PlayerInventoryRecipeV2,1,{pub format:u32,pub items:Vec<ItemDefinitionV1>,pub inventories:Vec<InventoryDefinitionV1>,pub starter:Vec<PlayerStarterItemV1>,pub consumables:Vec<PlayerConsumableV2>});

/// V2's additional storage is isolated so V1 retains its admission layout.
/// JSON is the selected recipe object, without an enum wrapper.
#[derive(Clone, PartialEq, Debug)]
pub enum PlayerInventoryRecipe {
    V1(PlayerInventoryRecipeV1),
    V2(Box<PlayerInventoryRecipeV2>),
}
impl From<PlayerInventoryRecipeV1> for PlayerInventoryRecipe {
    fn from(value: PlayerInventoryRecipeV1) -> Self {
        Self::V1(value)
    }
}
impl From<PlayerInventoryRecipeV2> for PlayerInventoryRecipe {
    fn from(value: PlayerInventoryRecipeV2) -> Self {
        Self::V2(Box::new(value))
    }
}
impl Serialize for PlayerInventoryRecipe {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::V1(v) => v.serialize(s),
            Self::V2(v) => v.serialize(s),
        }
    }
}
impl<'de> Deserialize<'de> for PlayerInventoryRecipe {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let _scope = InventoryDecodeScope::enter();
        // A named, order-independent map visitor. Each value uses the shared
        // bounded decoder; unknown/duplicate members never buffer raw Content.
        #[derive(Default)]
        struct Effects(Option<Vec<PlayerConsumableV2>>);
        impl<'de> Deserialize<'de> for Effects {
            fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
                Vec::field(d).map(|v| Self(Some(v)))
            }
        }
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            #[serde(deserialize_with = "field")]
            format: u32,
            #[serde(deserialize_with = "field")]
            items: Vec<ItemDefinitionV1>,
            #[serde(deserialize_with = "field")]
            inventories: Vec<InventoryDefinitionV1>,
            #[serde(deserialize_with = "field")]
            starter: Vec<PlayerStarterItemV1>,
            #[serde(default)]
            consumables: Effects,
        }
        let f: Fields = crate::json::object(d)?;
        let value = match (f.format, f.consumables.0) {
            (1, None) => PlayerInventoryRecipeV1 {
                format: f.format,
                items: f.items,
                inventories: f.inventories,
                starter: f.starter,
            }
            .into(),
            (2, Some(consumables)) => {
                charge::<D::Error>(size_of::<PlayerInventoryRecipeV2>())?;
                PlayerInventoryRecipeV2 {
                    format: f.format,
                    items: f.items,
                    inventories: f.inventories,
                    starter: f.starter,
                    consumables,
                }
                .into()
            }
            _ => {
                return Err(de::Error::custom(
                    "player inventory recipe version or fields invalid",
                ));
            }
        };
        finish_decode::<Self, D::Error>("PlayerInventoryRecipe", &value)?;
        Ok(value)
    }
}
impl Field for PlayerInventoryRecipe {
    fn field<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::deserialize(d)
    }
}
impl Graph for PlayerInventoryRecipe {
    const LIMIT: usize = 1;
    fn walk(&self, b: &mut Budget) -> Result<()> {
        match self {
            Self::V1(v) => v.walk(b),
            Self::V2(v) => {
                b.add(size_of::<PlayerInventoryRecipeV2>())?;
                v.walk(b)
            }
        }
    }
}
impl PlayerInventoryRecipe {
    pub(crate) fn items(&self) -> &Vec<ItemDefinitionV1> {
        match self {
            Self::V1(v) => &v.items,
            Self::V2(v) => &v.items,
        }
    }
    pub(crate) fn inventories(&self) -> &Vec<InventoryDefinitionV1> {
        match self {
            Self::V1(v) => &v.inventories,
            Self::V2(v) => &v.inventories,
        }
    }
    pub(crate) fn starter(&self) -> &Vec<PlayerStarterItemV1> {
        match self {
            Self::V1(v) => &v.starter,
            Self::V2(v) => &v.starter,
        }
    }
    pub(crate) fn effect(&self, item: &str) -> Option<&PlayerConsumableV2> {
        match self {
            Self::V1(_) => None,
            Self::V2(v) => v.consumables.iter().find(|e| e.item == item),
        }
    }
}
dto!(PlayerBagV1,4096,{pub owner:PlayerInventoryOwnerV1,pub revision:u64,pub consumed:Vec<ItemTotalV1>,pub inventories:Vec<InventoryStateV1>});
dto!(PlayerInventorySnapshotV1,1,{pub format:u32,pub recipe_id:String,pub recipe:PlayerInventoryRecipe,pub baseline:InventoryInitialTotalsV1,pub revision:u64,pub next_stack_id:u64,pub bags:Vec<PlayerBagV1>});

dto!(PlayerBagV2,4096,{pub owner:PlayerInventoryOwnerV1,pub revision:u64,pub consumed:Vec<ItemTotalV1>,pub received:Vec<ItemTotalV1>,pub sent:Vec<ItemTotalV1>,pub inventories:Vec<InventoryStateV1>});
dto!(PlayerInventorySnapshotV2,1,{pub format:u32,pub recipe_id:String,pub recipe:PlayerInventoryRecipe,pub baseline:InventoryInitialTotalsV1,pub revision:u64,pub next_stack_id:u64,pub bags:Vec<PlayerBagV2>});

/// The selected section object; V1 keeps its concrete durable layout.
#[derive(Clone, PartialEq, Debug)]
pub enum PlayerInventorySnapshot {
    V1(PlayerInventorySnapshotV1),
    V2(Box<PlayerInventorySnapshotV2>),
}
impl From<PlayerInventorySnapshotV1> for PlayerInventorySnapshot {
    fn from(v: PlayerInventorySnapshotV1) -> Self {
        Self::V1(v)
    }
}
impl From<PlayerInventorySnapshotV2> for PlayerInventorySnapshot {
    fn from(v: PlayerInventorySnapshotV2) -> Self {
        Self::V2(Box::new(v))
    }
}
impl Serialize for PlayerInventorySnapshot {
    fn serialize<S: serde::Serializer>(&self, s: S) -> std::result::Result<S::Ok, S::Error> {
        match self {
            Self::V1(v) => v.serialize(s),
            Self::V2(v) => v.serialize(s),
        }
    }
}
// Presence is distinct from an empty array, and null never means absent.
#[derive(Default)]
struct FlowField(Option<Vec<ItemTotalV1>>);
impl<'de> Deserialize<'de> for FlowField {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Vec::field(d).map(|v| Self(Some(v)))
    }
}
enum BagRow {
    V1(PlayerBagV1),
    V2(PlayerBagV2),
}
impl<'de> Deserialize<'de> for BagRow {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            #[serde(deserialize_with = "field")]
            owner: PlayerInventoryOwnerV1,
            #[serde(deserialize_with = "field")]
            revision: u64,
            #[serde(deserialize_with = "field")]
            consumed: Vec<ItemTotalV1>,
            #[serde(default)]
            received: FlowField,
            #[serde(default)]
            sent: FlowField,
            #[serde(deserialize_with = "field")]
            inventories: Vec<InventoryStateV1>,
        }
        let f: Fields = crate::json::object(d)?;
        match (f.received.0, f.sent.0) {
            (None, None) => Ok(Self::V1(PlayerBagV1 {
                owner: f.owner,
                revision: f.revision,
                consumed: f.consumed,
                inventories: f.inventories,
            })),
            (Some(received), Some(sent)) => Ok(Self::V2(PlayerBagV2 {
                owner: f.owner,
                revision: f.revision,
                consumed: f.consumed,
                received,
                sent,
                inventories: f.inventories,
            })),
            _ => Err(de::Error::custom(
                "player inventory accounting fields invalid",
            )),
        }
    }
}
enum BagRows {
    Empty,
    V1(Vec<PlayerBagV1>),
    V2(Vec<PlayerBagV2>),
}
fn push_bag<T: Graph, E: de::Error>(v: &mut Vec<T>, item: T) -> std::result::Result<(), E> {
    if v.len() >= T::LIMIT {
        return Err(E::custom("inventory sequence limit"));
    }
    if v.len() == v.capacity() {
        let cap = next_decode_capacity::<T>(v.capacity())
            .map_err(|_| E::custom("inventory sequence limit"))?;
        charge::<E>(
            (cap - v.capacity())
                .checked_mul(size_of::<T>())
                .ok_or_else(|| E::custom("inventory allocation limit"))?,
        )?;
        v.try_reserve_exact(cap - v.len())
            .map_err(|_| E::custom("inventory allocation limit"))?;
    }
    v.push(item);
    Ok(())
}
impl<'de> Deserialize<'de> for BagRows {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        struct V;
        impl<'de> de::Visitor<'de> for V {
            type Value = BagRows;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("bounded player bags")
            }
            fn visit_seq<A: de::SeqAccess<'de>>(
                self,
                mut a: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                struct Seed(usize);
                impl<'de> de::DeserializeSeed<'de> for Seed {
                    type Value = BagRow;
                    fn deserialize<D: Deserializer<'de>>(
                        self,
                        d: D,
                    ) -> std::result::Result<BagRow, D::Error> {
                        if self.0 >= 4096 {
                            return Err(de::Error::custom("inventory sequence limit"));
                        }
                        BagRow::deserialize(d)
                    }
                }
                let mut rows = BagRows::Empty;
                let mut count = 0;
                while let Some(row) = a.next_element_seed(Seed(count))? {
                    rows = match (rows, row) {
                        (BagRows::Empty, BagRow::V1(v)) => {
                            let mut rows = Vec::new();
                            push_bag::<_, A::Error>(&mut rows, v)?;
                            BagRows::V1(rows)
                        }
                        (BagRows::Empty, BagRow::V2(v)) => {
                            let mut rows = Vec::new();
                            push_bag::<_, A::Error>(&mut rows, v)?;
                            BagRows::V2(rows)
                        }
                        (BagRows::V1(mut rows), BagRow::V1(v)) => {
                            push_bag::<_, A::Error>(&mut rows, v)?;
                            BagRows::V1(rows)
                        }
                        (BagRows::V2(mut rows), BagRow::V2(v)) => {
                            push_bag::<_, A::Error>(&mut rows, v)?;
                            BagRows::V2(rows)
                        }
                        _ => return Err(de::Error::custom("mixed player inventory bag formats")),
                    };
                    count += 1;
                }
                Ok(rows)
            }
        }
        d.deserialize_seq(V)
    }
}
impl<'de> Deserialize<'de> for PlayerInventorySnapshot {
    fn deserialize<D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        let _scope = InventoryDecodeScope::enter();
        #[derive(Deserialize)]
        #[serde(deny_unknown_fields)]
        struct Fields {
            #[serde(deserialize_with = "field")]
            format: u32,
            #[serde(deserialize_with = "field")]
            recipe_id: String,
            #[serde(deserialize_with = "field")]
            recipe: PlayerInventoryRecipe,
            #[serde(deserialize_with = "field")]
            baseline: InventoryInitialTotalsV1,
            #[serde(deserialize_with = "field")]
            revision: u64,
            #[serde(deserialize_with = "field")]
            next_stack_id: u64,
            bags: BagRows,
        }
        let f: Fields = crate::json::object(d)
            .map_err(|_| de::Error::custom("player inventory object invalid"))?;
        let rows = match (f.format, f.bags) {
            (1, BagRows::Empty) => BagRows::V1(Vec::new()),
            (2, BagRows::Empty) => BagRows::V2(Vec::new()),
            (_, v) => v,
        };
        let value = match (f.format, rows) {
            (1, BagRows::V1(bags)) => PlayerInventorySnapshotV1 {
                format: 1,
                recipe_id: f.recipe_id,
                recipe: f.recipe,
                baseline: f.baseline,
                revision: f.revision,
                next_stack_id: f.next_stack_id,
                bags,
            }
            .into(),
            (2, BagRows::V2(bags)) => {
                charge::<D::Error>(size_of::<PlayerInventorySnapshotV2>())?;
                PlayerInventorySnapshotV2 {
                    format: 2,
                    recipe_id: f.recipe_id,
                    recipe: f.recipe,
                    baseline: f.baseline,
                    revision: f.revision,
                    next_stack_id: f.next_stack_id,
                    bags,
                }
                .into()
            }
            _ => {
                return Err(de::Error::custom(
                    "player inventory format or fields invalid",
                ));
            }
        };
        finish_decode::<Self, D::Error>("PlayerInventorySnapshot", &value)?;
        Ok(value)
    }
}
impl Field for PlayerInventorySnapshot {
    fn field<'de, D: Deserializer<'de>>(d: D) -> std::result::Result<Self, D::Error> {
        Self::deserialize(d)
    }
}
impl Graph for PlayerInventorySnapshot {
    const LIMIT: usize = 1;
    fn walk(&self, b: &mut Budget) -> Result<()> {
        match self {
            Self::V1(v) => v.walk(b),
            Self::V2(v) => {
                b.add(size_of::<PlayerInventorySnapshotV2>())?;
                v.walk(b)
            }
        }
    }
}
pub(crate) fn player_inventory_v2_bytes(v: &PlayerInventorySnapshotV2) -> Result<usize> {
    graph(v)?
        .checked_add(size_of::<PlayerInventorySnapshot>())
        .ok_or_else(|| anyhow::anyhow!("inventory allocation limit"))
}
pub(crate) fn player_inventory_v2_charge(
    v: &PlayerInventorySnapshotV2,
    copies: usize,
) -> Result<()> {
    let pins = graph(&v.baseline)?
        .checked_add(64)
        .and_then(|n| {
            n.checked_add(crate::native_player_runtime::inventory_pin_v2_overhead_bytes())
        })
        .and_then(|n| n.checked_mul(128));
    // Four wide sums, three dense total vectors with maximum item strings,
    // and two bounded endpoint plans. All remain inside the existing cap.
    let scratch = MAX_INVENTORY_ITEMS
        .checked_mul(4 * size_of::<u128>() + 3 * (size_of::<ItemTotalV1>() + 64))
        .and_then(|n| n.checked_add(2 * 1024 + 64 * 1024));
    ensure!(
        player_inventory_v2_bytes(v)?
            .checked_mul(copies)
            .and_then(|n| pins.and_then(|p| n.checked_add(p)))
            .and_then(|n| scratch.and_then(|p| n.checked_add(p)))
            .is_some_and(|n| n <= MAX_INVENTORY_GRAPH_BYTES),
        "player inventory allocation limit"
    );
    encoded(v, MAX_INVENTORY_STATE_BYTES)?;
    Ok(())
}
dto!(PlayerStackEndpointV1,1,{pub owner:PlayerInventoryOwnerV1,pub inventory:String,pub stack_id:StackId});
dto!(PlayerSlotEndpointV1,1,{pub owner:PlayerInventoryOwnerV1,pub inventory:String,pub slot:u16});
dto!(PlayerInventoryRevisionPairV1,1,{pub expected_player_revision:u64,pub expected_bag_revision:u64});
dto!(PlayerInventoryTransferRequestV1,1,{pub source:PlayerStackEndpointV1,pub destination:PlayerSlotEndpointV1,pub amount:u32,pub source_cas:PlayerInventoryRevisionPairV1,pub destination_cas:PlayerInventoryRevisionPairV1});
dto!(PlayerOverflowDestinationV1,1,{pub owner:PlayerInventoryOwnerV1,pub inventory:String,pub slot:Option<u16>});

pub(crate) fn player_action_bounds(value: &impl Serialize) -> Result<()> {
    encoded(value, 64 * 1024)?;
    Ok(())
}
pub(crate) fn player_inventory_bytes(value: &PlayerInventorySnapshotV1) -> Result<usize> {
    graph(value)
}
pub(crate) fn player_inventory_charge(
    value: &PlayerInventorySnapshotV1,
    copies: usize,
) -> Result<()> {
    let bytes = graph(value)?;
    // Reserve both cutoff and in-flight submission pins for all 64 possible
    // leaves, including their independently cloned baseline vectors. Bag
    // contents themselves are covered by the retained aggregate copies.
    let pins = graph(&value.baseline)?
        .checked_add(64)
        .and_then(|n| n.checked_mul(128));
    ensure!(
        bytes
            .checked_mul(copies)
            .and_then(|n| pins.and_then(|p| n.checked_add(p)))
            .is_some_and(|n| n <= MAX_INVENTORY_GRAPH_BYTES),
        "player inventory allocation limit"
    );
    encoded(value, MAX_INVENTORY_STATE_BYTES)?;
    Ok(())
}
pub(crate) fn player_recipe_charge(value: &PlayerInventoryRecipe) -> Result<()> {
    graph(value)?;
    encoded(value, MAX_INVENTORY_CONTRACT_BYTES)?;
    Ok(())
}
// Reproducible authored baseline, independent of preserved save provenance.
dto!(AuthoredInventoryAuthorityV1,1,{pub format:u32,pub lifecycle_id:String,pub initial_owners:InventoryLiveOwnersV1,pub initial_items:InitialInventoryItemsV1,pub initial_state_digest:String,pub initial_next_stack_id:u64,pub initial_totals:InventoryInitialTotalsV1});
dto!(InventoryStackEndpointV1,1,{pub owner:OwnerHandle,pub inventory:String,pub stack_id:StackId});
dto!(InventorySlotEndpointV1,1,{pub owner:OwnerHandle,pub inventory:String,pub slot:u16});
// An explicit null slot appends one eligible next slot; a number names an
// existing allocated slot. All fields are required when decoding.
dto!(InventoryOverflowDestinationV1,1,{pub owner:OwnerHandle,pub inventory:String,pub slot:Option<u16>});

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
    /// Transfers an overflow stack to the explicit destination. None appends
    /// one eligible next slot; Some requires an empty allocated slot.
    PlaceOverflow {
        source: InventoryStackEndpointV1,
        destination: InventoryOverflowDestinationV1,
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
            slot: Arg<Option<u16>>,
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
        }
        let f: Fields = crate::json::object(d)
            .map_err(|_| <D::Error as de::Error>::custom("inventory action invalid"))?;
        let invalid = || <D::Error as de::Error>::custom("inventory action invalid");
        let source = f.source;
        match f.kind.as_str() {
            "move" | "split" => {
                let d = f.destination.0.ok_or_else(invalid)?;
                if d.stack_id.0.is_some() {
                    return Err(invalid());
                }
                let destination = InventorySlotEndpointV1 {
                    owner: d.owner,
                    inventory: d.inventory,
                    slot: d.slot.0.flatten().ok_or_else(invalid)?,
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
                if f.destination.0.is_some() {
                    return Err(invalid());
                }
                Ok(Self::Consume {
                    source,
                    amount: f.amount.0.ok_or_else(invalid)?,
                })
            }
            "place_overflow" => {
                if f.amount.0.is_some() {
                    return Err(invalid());
                }
                let d = f.destination.0.ok_or_else(invalid)?;
                if d.stack_id.0.is_some() {
                    return Err(invalid());
                }
                Ok(Self::PlaceOverflow {
                    source,
                    destination: InventoryOverflowDestinationV1 {
                        owner: d.owner,
                        inventory: d.inventory,
                        slot: d.slot.0.ok_or_else(invalid)?,
                    },
                })
            }
            _ => Err(invalid()),
        }
    }
}
impl<'de> Deserialize<'de> for crate::native_player_inventory::PlayerInventoryActionV1 {
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
            owner: PlayerInventoryOwnerV1,
            #[serde(deserialize_with = "field")]
            inventory: String,
            #[serde(default)]
            slot: Arg<Option<u16>>,
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
            source: PlayerStackEndpointV1,
            #[serde(default)]
            destination: Arg<Destination>,
            #[serde(default)]
            amount: Arg<u32>,
        }
        let f: Fields = crate::json::object(d)
            .map_err(|_| <D::Error as de::Error>::custom("inventory action invalid"))?;
        let invalid = || <D::Error as de::Error>::custom("inventory action invalid");
        let source = f.source;
        match f.kind.as_str() {
            "move" | "split" => {
                let d = f.destination.0.ok_or_else(invalid)?;
                if d.stack_id.0.is_some() {
                    return Err(invalid());
                }
                let destination = PlayerSlotEndpointV1 {
                    owner: d.owner,
                    inventory: d.inventory,
                    slot: d.slot.0.flatten().ok_or_else(invalid)?,
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
                let d = f.destination.0.ok_or_else(invalid)?;
                if d.slot.0.is_some() {
                    return Err(invalid());
                }
                let destination = PlayerStackEndpointV1 {
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
            "use" => {
                if f.destination.0.is_some() || f.amount.0.is_some() {
                    return Err(invalid());
                }
                Ok(Self::Use { source })
            }
            "consume" => {
                if f.destination.0.is_some() {
                    return Err(invalid());
                }
                Ok(Self::Consume {
                    source,
                    amount: f.amount.0.ok_or_else(invalid)?,
                })
            }
            "place_overflow" => {
                if f.amount.0.is_some() {
                    return Err(invalid());
                }
                let d = f.destination.0.ok_or_else(invalid)?;
                if d.stack_id.0.is_some() {
                    return Err(invalid());
                }
                Ok(Self::PlaceOverflow {
                    source,
                    destination: PlayerOverflowDestinationV1 {
                        owner: d.owner,
                        inventory: d.inventory,
                        slot: d.slot.0.ok_or_else(invalid)?,
                    },
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
            Self::PlaceOverflow {
                source,
                destination,
            } => {
                source.walk(b)?;
                destination.walk(b)?;
            }
            Self::Consume { source, .. } => source.walk(b)?,
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

impl InitialInventoryItemsV1 {
    pub fn load(path: &Path) -> Result<Self> {
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(crate::nonblocking_flag())
            .open(path)
            .map_err(|_| anyhow::anyhow!("initial inventory open failed"))?;
        let meta = file
            .metadata()
            .map_err(|_| anyhow::anyhow!("initial inventory metadata failed"))?;
        ensure!(
            meta.is_file() && meta.len() <= MAX_INVENTORY_STATE_BYTES as u64,
            "initial inventory regular file or size invalid"
        );
        let mut text = String::new();
        file.take(MAX_INVENTORY_STATE_BYTES as u64 + 1)
            .read_to_string(&mut text)
            .map_err(|_| anyhow::anyhow!("initial inventory read failed"))?;
        ensure!(
            text.len() <= MAX_INVENTORY_STATE_BYTES,
            "initial inventory size invalid"
        );
        serde_json::from_str(&text).map_err(|_| anyhow::anyhow!("initial inventory JSON invalid"))
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
fn validate_inventory_rules<'a>(
    definition: &'a InventoryContractsV1,
    used: &BTreeSet<&str>,
) -> Result<(
    BTreeSet<&'a str>,
    BTreeMap<&'a str, &'a InventoryOwnerClassV1>,
)> {
    let mut items = BTreeSet::new();
    for item in &definition.items {
        ensure!(
            items.insert(item.key.as_str())
                && item.stack_limit > 0
                && item.stack_limit <= MAX_INVENTORY_STACK_COUNT,
            "inventory item definition invalid"
        );
    }
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
    Ok((items, classes))
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
        let used: BTreeSet<_> = map.placements.iter().map(|p| p.class.as_str()).collect();
        let (items, classes) = validate_inventory_rules(&definition, &used)?;
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
    pub(crate) fn player_bound(
        recipe: &PlayerInventoryRecipe,
        ids: impl Iterator<Item = u64>,
        identity: &str,
    ) -> Result<Self> {
        player_recipe_charge(recipe)?;
        ensure!(
            !recipe.inventories().is_empty(),
            "player inventory recipe invalid"
        );
        let definition = InventoryContractsV1 {
            format: 1,
            native_catalog_id: identity.to_owned(),
            native_map: "player".into(),
            items: recipe.items().clone(),
            classes: vec![InventoryOwnerClassV1 {
                native_class: "player".into(),
                inventories: recipe.inventories().clone(),
            }],
        };
        validate_inventory_rules(&definition, &BTreeSet::from(["player"]))?;
        let mut bindings = BTreeMap::new();
        for id in ids {
            ensure!(
                id > 0 && bindings.insert(DomainId(id), 0).is_none() && bindings.len() <= 4096,
                "player owner binding invalid"
            );
        }
        Ok(Self {
            snapshot: Arc::new(InventoryContractsSnapshotV1 {
                contract_id: identity.to_owned(),
                definition,
            }),
            bindings,
        })
    }
    /// Narrow a validated contract to one existing owner without recompiling
    /// its immutable recipe. Live-owner validation still requires exact coverage.
    pub(crate) fn for_owner(&self, id: DomainId) -> Result<Self> {
        let class = *self
            .bindings
            .get(&id)
            .ok_or_else(|| anyhow::anyhow!("inventory owner binding missing"))?;
        Ok(Self {
            snapshot: Arc::clone(&self.snapshot),
            bindings: BTreeMap::from([(id, class)]),
        })
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
        let recipes = self.recipes(id)?;
        recipes
            .binary_search_by(|recipe| recipe.key.as_str().cmp(key))
            .map(|index| &recipes[index])
            .map_err(|_| anyhow::anyhow!("inventory instance missing"))
    }
    fn limit(&self, item: &str) -> Result<u32> {
        let items = &self.definition().items;
        items
            .binary_search_by(|definition| definition.key.as_str().cmp(item))
            .map(|index| items[index].stack_limit)
            .map_err(|_| anyhow::anyhow!("inventory item undeclared"))
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
    recipe.slot_rules.get(slot).is_some_and(|r| {
        r.allowed_items
            .binary_search_by(|key| key.as_str().cmp(item))
            .is_ok()
    })
}
fn input_budget(values: &[&dyn InventoryGraph]) -> Result<usize> {
    let mut b = Budget::default();
    for v in values {
        v.inventory_walk(&mut b)?;
    }
    Ok(b.0)
}

/// Simultaneous inventory definitions/state allowance, including decoder growth.
/// Callers add the independently checked provenance allowance before admission.
pub(crate) fn inventory_snapshot_allocation_bytes(
    contracts: &InventoryContractsSnapshotV1,
    state: &InventoryStateSnapshotV1,
) -> Result<usize> {
    contracts.validate_bounds()?;
    state.validate_bounds()?;
    input_budget(&[contracts, state])
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
    validate_inventory_state_target(contracts, state, initial_totals, live, None)
}
fn validate_inventory_state_target(
    contracts: &CompiledInventoryContractsV1,
    state: &InventoryStateSnapshotV1,
    initial_totals: &InventoryInitialTotalsV1,
    live: &InventoryLiveOwnersV1,
    wide_target: Option<&[u128]>,
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
    enum Quantities<'a> {
        Legacy(BTreeMap<&'a str, u64>),
        Wide(Vec<u128>),
    }
    let mut quantities = if wide_target.is_some() {
        // V2's adapter is dense in immutable recipe order. Its four wide
        // vectors (global receive/send, target, contents+consumed) have an
        // explicit reservation; no new accounting tree allocation is needed.
        ensure!(
            initial_totals
                .items
                .windows(2)
                .all(|r| r[0].item < r[1].item)
                && state
                    .consumed
                    .iter()
                    .map(|r| r.item.as_str())
                    .eq(initial.keys().copied()),
            "inventory consumed coverage invalid"
        );
        Quantities::Wide(state.consumed.iter().map(|r| u128::from(r.count)).collect())
    } else {
        let mut quantities = BTreeMap::new();
        for v in &state.consumed {
            ensure!(
                initial.contains_key(v.item.as_str())
                    && quantities.insert(v.item.as_str(), v.count).is_none(),
                "inventory consumed total invalid"
            );
        }
        Quantities::Legacy(quantities)
    };
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
                match &mut quantities {
                    Quantities::Legacy(quantities) => {
                        let q = quantities
                            .get_mut(s.item.as_str())
                            .ok_or_else(|| anyhow::anyhow!("inventory quantity item missing"))?;
                        add(q, u64::from(s.count))?;
                    }
                    Quantities::Wide(quantities) => {
                        let index = initial_totals
                            .items
                            .binary_search_by(|v| v.item.cmp(&s.item))
                            .map_err(|_| anyhow::anyhow!("inventory quantity item missing"))?;
                        quantities[index] = quantities[index]
                            .checked_add(u128::from(s.count))
                            .ok_or_else(|| anyhow::anyhow!("inventory quantity overflow"))?;
                    }
                }
                Ok(())
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
    match (quantities, wide_target) {
        (Quantities::Legacy(quantities), None) => {
            ensure!(quantities == initial, "inventory conservation mismatch")
        }
        (Quantities::Wide(quantities), Some(target)) => {
            ensure!(quantities == target, "inventory conservation mismatch")
        }
        _ => unreachable!("selected inventory accounting width"),
    }
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
    prepare_initial_inventory_retaining(contracts, live, items, 0)
}

fn prepare_initial_inventory_retaining(
    contracts: &CompiledInventoryContractsV1,
    live: &InventoryLiveOwnersV1,
    items: &InitialInventoryItemsV1,
    retained_bytes: usize,
) -> Result<PreparedInitialInventoryV1> {
    prepare_initial_inventory_from(contracts, live, items, retained_bytes, 1)
}

pub(crate) fn prepare_initial_inventory_from(
    contracts: &CompiledInventoryContractsV1,
    live: &InventoryLiveOwnersV1,
    items: &InitialInventoryItemsV1,
    retained_bytes: usize,
    start: u64,
) -> Result<PreparedInitialInventoryV1> {
    ensure!(start > 0, "inventory allocator invalid");
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
    let mut retained = Budget(retained_bytes);
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
            .and_then(|n| n.checked_add(start))
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
        let owner_index = state
            .owners
            .binary_search_by_key(&item.owner, |o| o.owner.id)
            .expect("validated owner");
        let owner = &mut state.owners[owner_index];
        let inventory_index = owner
            .inventories
            .binary_search_by(|i| i.key.cmp(&item.inventory))
            .expect("validated inventory");
        let inv = &mut owner.inventories[inventory_index];
        let slot = usize::from(item.requested_slot);
        let accepted =
            inv.slots.get(slot).is_some_and(Option::is_none) && eligible(recipe, slot, &item.item);
        let stack = StackV1 {
            id: StackId(index as u64 + start),
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
    prepare_inventory_action_target(contracts, current, initial_totals, live, request, None)
}
fn prepare_inventory_action_target(
    contracts: &CompiledInventoryContractsV1,
    current: &InventoryStateSnapshotV1,
    initial_totals: &InventoryInitialTotalsV1,
    live: &InventoryLiveOwnersV1,
    request: &InventoryActionRequestV1,
    target: Option<&[u128]>,
) -> Result<PreparedInventoryStateV1> {
    let admitted_bytes = input_budget(&[current, initial_totals, live, request])?;
    validate_inventory_state_target(contracts, current, initial_totals, live, target)?;
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
        Place(Address, Address, Option<usize>),
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
        InventoryActionV1::PlaceOverflow {
            source,
            destination: dest,
        } => {
            let s = stack_address(current, source)?;
            ensure!(
                matches!(s.location, Location::Overflow(_)),
                "inventory overflow endpoint required"
            );
            let (owner, inventory) = inventory_address(current, dest.owner, &dest.inventory)?;
            let inv = &current.owners[owner].inventories[inventory];
            let index = dest.slot.map_or(inv.slots.len(), usize::from);
            ensure!(
                (dest.slot.is_none() || inv.slots.get(index).is_some_and(Option::is_none))
                    && eligible(
                        contracts.recipe(dest.owner.id, &dest.inventory)?,
                        index,
                        &at(current, s).item
                    ),
                "inventory overflow placement refused"
            );
            let capacity = if dest.slot.is_none() {
                Some(decode_capacity::<Option<StackV1>>(index + 1)?)
            } else {
                None
            };
            Plan::Place(
                s,
                Address {
                    owner,
                    inventory,
                    location: Location::Slot(index),
                },
                capacity,
            )
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
        Plan::Place(_, d, Some(capacity)) => {
            let slots = &current.owners[d.owner].inventories[d.inventory].slots;
            let before = slots
                .capacity()
                .max(decode_capacity::<Option<StackV1>>(slots.len())?);
            let after = slots.capacity().max(capacity);
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
    // Stage the only fallible placement allocation before cloning or removing
    // the source. The commit fills this vector without further growth.
    let mut appended_slots = if let Plan::Place(_, _, Some(capacity)) = plan {
        let mut slots = Vec::new();
        slots
            .try_reserve_exact(capacity)
            .map_err(|_| anyhow::anyhow!("inventory allocation limit"))?;
        Some(slots)
    } else {
        None
    };
    let mut state = current.clone();
    let placement = matches!(plan, Plan::Place(..));
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
        Plan::Place(s, d, capacity) => {
            let stack = remove(&mut state, s);
            if capacity.is_some() {
                let inv = &mut state.owners[d.owner].inventories[d.inventory];
                let mut slots = appended_slots.take().expect("staged slot allocation");
                slots.append(&mut inv.slots);
                slots.push(Some(stack));
                inv.slots = slots;
            } else {
                install(&mut state, d, stack);
            }
        }
    }
    state.revision = next_revision;
    if !placement {
        validate_inventory_state_target(contracts, &state, initial_totals, live, target)?;
    }
    // Placement's complete checks precede mutation: it transfers the unchanged
    // stack, preserves coverage/allocator/totals, and grows only the admitted
    // destination slots. Removing the overflow wrapper saves more JSON bytes
    // than a slot append and any revision digit growth can add, so the input's
    // encoding admission covers the result too. No fallible placement commit.
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

impl AuthoredInventoryAuthorityV1 {
    pub(crate) fn allocation_bytes(&self) -> Result<usize> {
        graph(self)
    }

    pub(crate) fn validate_current(
        &self,
        contracts: &CompiledInventoryContractsV1,
        lifecycle: &crate::native_lifecycle::LifecycleSnapshot,
        state: &InventoryStateSnapshotV1,
    ) -> Result<()> {
        self.allocation_bytes()?;
        ensure!(
            contracts.definition().native_map == lifecycle.definition.map,
            "authored inventory map differs from lifecycle"
        );
        ensure!(
            self.format == 1 && self.lifecycle_id == lifecycle.lifecycle_id,
            "authored inventory authority identity invalid"
        );
        let live = authored_inventory_live_owners(lifecycle)?;
        ensure!(
            live == self.initial_owners,
            "authored inventory owner identities changed"
        );
        ensure!(
            valid_digest(&self.initial_state_digest)
                && self.initial_next_stack_id > 0
                && state.next_stack_id >= self.initial_next_stack_id,
            "authored inventory allocator baseline invalid"
        );
        validate_inventory_state(contracts, state, &self.initial_totals, &live)
    }

    pub(crate) fn validate(
        &self,
        contracts: &CompiledInventoryContractsV1,
        declarations: &InventoryContractsSnapshotV1,
        lifecycle: &crate::native_lifecycle::LifecycleSnapshot,
        state: &InventoryStateSnapshotV1,
    ) -> Result<()> {
        let retained = authored_inventory_allocation_bytes(declarations, state, self)?;
        self.validate_current(contracts, lifecycle, state)?;
        let initial = prepare_initial_inventory_retaining(
            contracts,
            &self.initial_owners,
            &self.initial_items,
            retained,
        )?;
        ensure!(
            initial.initial_state_digest == self.initial_state_digest
                && initial.state.next_stack_id == self.initial_next_stack_id
                && initial.initial_totals == self.initial_totals,
            "authored inventory initial receipt invalid"
        );
        Ok(())
    }
}

pub(crate) fn authored_inventory_allocation_bytes(
    contracts: &InventoryContractsSnapshotV1,
    state: &InventoryStateSnapshotV1,
    authority: &AuthoredInventoryAuthorityV1,
) -> Result<usize> {
    contracts.validate_bounds()?;
    state.validate_bounds()?;
    input_budget(&[contracts, state, authority])
}

fn authored_inventory_live_owners(
    lifecycle: &crate::native_lifecycle::LifecycleSnapshot,
) -> Result<InventoryLiveOwnersV1> {
    ensure!(
        lifecycle
            .slots
            .iter()
            .all(|slot| slot.presence == crate::native_lifecycle::Presence::Live),
        "authored inventory owners must all be live"
    );
    let mut owners: Vec<_> = lifecycle
        .slots
        .iter()
        .map(|slot| OwnerHandle {
            id: slot.id,
            incarnation: slot.incarnation,
        })
        .collect();
    owners.sort_by_key(|owner| owner.id);
    Ok(InventoryLiveOwnersV1 { owners })
}

/// Explicit opt-in from a complete independently authored schema-7 world.
/// The retained recipe is the conservation baseline; no save input is needed.
pub fn enable_inventory_owners(
    mut snapshot: crate::simulation::WorldSnapshot,
    declarations: InventoryContractsSnapshotV1,
    items: InitialInventoryItemsV1,
) -> Result<crate::simulation::WorldSnapshot> {
    ensure!(
        snapshot.schema_version == crate::simulation::SNAPSHOT_SCHEMA_LIFECYCLE,
        "inventory opt-in requires authored lifecycle schema 7"
    );
    snapshot.validate()?;
    let native = snapshot
        .native_world
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("inventory spatial state missing"))?;
    let lifecycle = snapshot
        .native_lifecycle
        .as_ref()
        .ok_or_else(|| anyhow::anyhow!("inventory lifecycle missing"))?;
    let catalog = CompiledNativeCatalog::compile(native.definitions.clone())?;
    let contracts = CompiledInventoryContractsV1::from_snapshot(&declarations, &catalog)?;
    let live = authored_inventory_live_owners(lifecycle)?;
    let retained = input_budget(&[&declarations, &items, &live])?;
    let prepared = prepare_initial_inventory_retaining(&contracts, &live, &items, retained)?;
    let (state, initial_totals, initial_state_digest) = prepared.into_parts();
    let authority = AuthoredInventoryAuthorityV1 {
        format: 1,
        lifecycle_id: lifecycle.lifecycle_id.clone(),
        initial_owners: live,
        initial_items: items,
        initial_state_digest,
        initial_next_stack_id: state.next_stack_id,
        initial_totals,
    };
    authored_inventory_allocation_bytes(&declarations, &state, &authority)?;
    snapshot.schema_version = crate::simulation::SNAPSHOT_SCHEMA_INVENTORY_OWNERS;
    snapshot.native_inventory_contracts = Some(declarations);
    snapshot.native_inventories = Some(state);
    snapshot.native_inventory_authority = Some(authority);
    snapshot.validate()?;
    Ok(snapshot)
}

// The wide target is derived exclusively from immutable starter and retained
// flow arrays. It is never a proposed contents total or a public bypass flag.
pub(crate) fn player_inventory_wide_target(
    baseline: &InventoryInitialTotalsV1,
    received: &[ItemTotalV1],
    sent: &[ItemTotalV1],
) -> Result<Vec<u128>> {
    let mut target = baseline
        .items
        .iter()
        .map(|v| u128::from(v.count))
        .collect::<Vec<_>>();
    for (rows, subtract) in [(received, false), (sent, true)] {
        validate_player_sparse(baseline, rows)?;
        for row in rows {
            let index = baseline
                .items
                .binary_search_by(|v| v.item.cmp(&row.item))
                .map_err(|_| anyhow::anyhow!("player inventory item invalid"))?;
            target[index] = if subtract {
                target[index].checked_sub(u128::from(row.count))
            } else {
                target[index].checked_add(u128::from(row.count))
            }
            .ok_or_else(|| anyhow::anyhow!("player inventory balance invalid"))?;
        }
    }
    Ok(target)
}
pub(crate) fn validate_player_sparse(
    baseline: &InventoryInitialTotalsV1,
    rows: &[ItemTotalV1],
) -> Result<()> {
    ensure!(
        rows.len() <= MAX_INVENTORY_ITEMS && rows.windows(2).all(|r| r[0].item < r[1].item),
        "player inventory sparse order invalid"
    );
    for row in rows {
        ensure!(
            row.count > 0
                && baseline
                    .items
                    .binary_search_by(|v| v.item.cmp(&row.item))
                    .is_ok(),
            "player inventory sparse item invalid"
        );
    }
    Ok(())
}
pub(crate) fn validate_player_inventory_state_v2(
    contracts: &CompiledInventoryContractsV1,
    state: &InventoryStateSnapshotV1,
    baseline: &InventoryInitialTotalsV1,
    live: &InventoryLiveOwnersV1,
    received: &[ItemTotalV1],
    sent: &[ItemTotalV1],
) -> Result<()> {
    let target = player_inventory_wide_target(baseline, received, sent)?;
    validate_inventory_state_target(contracts, state, baseline, live, Some(&target))
}
pub(crate) fn prepare_player_inventory_action_v2(
    contracts: &CompiledInventoryContractsV1,
    state: &InventoryStateSnapshotV1,
    baseline: &InventoryInitialTotalsV1,
    live: &InventoryLiveOwnersV1,
    flows: (&[ItemTotalV1], &[ItemTotalV1]),
    request: &InventoryActionRequestV1,
) -> Result<PreparedInventoryStateV1> {
    let target = player_inventory_wide_target(baseline, flows.0, flows.1)?;
    let result =
        prepare_inventory_action_target(contracts, state, baseline, live, request, Some(&target))?;
    validate_inventory_state_target(contracts, result.state(), baseline, live, Some(&target))?;
    Ok(result)
}

#[cfg(test)]
mod authored_authority_budget_tests {
    use super::*;

    #[test]
    fn authority_and_current_graph_share_decode_budget_in_either_order() {
        let owner = OwnerInventoriesV1 {
            owner: OwnerHandle {
                id: DomainId(1),
                incarnation: 1,
            },
            inventories: (0..6)
                .map(|index| InventoryStateV1 {
                    key: format!("bag{index}"),
                    slots: vec![None; 128],
                    overflow: vec![],
                })
                .collect(),
        };
        let state = InventoryStateSnapshotV1 {
            format: 1,
            contract_id: "a".repeat(64),
            revision: 1,
            next_stack_id: 1,
            consumed: vec![],
            owners: vec![owner; 1900],
        };
        let authority = AuthoredInventoryAuthorityV1 {
            format: 1,
            lifecycle_id: "b".repeat(64),
            initial_owners: InventoryLiveOwnersV1 { owners: vec![] },
            initial_items: InitialInventoryItemsV1 {
                items: vec![
                    InitialInventoryItemV1 {
                        owner: DomainId(1),
                        inventory: "c".repeat(64),
                        requested_slot: 0,
                        item: "d".repeat(64),
                        count: 1,
                    };
                    65536
                ],
            },
            initial_state_digest: "e".repeat(64),
            initial_next_stack_id: 1,
            initial_totals: InventoryInitialTotalsV1 { items: vec![] },
        };
        // Structural budget fixtures only: semantics are covered by authored
        // world tests. Each encoding is independently admitted, but not jointly.
        state.validate_bounds().unwrap();
        authority.allocation_bytes().unwrap();
        assert!(input_budget(&[&state, &authority]).is_err());
        let state_json = serde_json::to_string(&state).unwrap();
        let authority_json = serde_json::to_string(&authority).unwrap();
        serde_json::from_str::<InventoryStateSnapshotV1>(&state_json).unwrap();
        serde_json::from_str::<AuthoredInventoryAuthorityV1>(&authority_json).unwrap();
        #[derive(Deserialize)]
        struct Joint {
            #[serde(rename = "state")]
            _state: InventoryStateSnapshotV1,
            #[serde(rename = "authority")]
            _authority: AuthoredInventoryAuthorityV1,
        }
        for json in [
            format!("{{\"state\":{state_json},\"authority\":{authority_json}}}"),
            format!("{{\"authority\":{authority_json},\"state\":{state_json}}}"),
        ] {
            {
                let _scope = InventoryDecodeScope::enter();
                assert!(serde_json::from_str::<Joint>(&json).is_err());
            }
            // A rejected joint request must release every thread-local charge.
            serde_json::from_str::<AuthoredInventoryAuthorityV1>(&authority_json).unwrap();
        }
        let mut spare = authority.clone();
        spare
            .initial_items
            .items
            .reserve(2 * MAX_INVENTORY_STACKS + 5);
        assert!(spare.allocation_bytes().is_err());
    }
}

#[cfg(test)]
mod player_recipe_layout_tests {
    use super::*;

    // Same field types and order as the pre-versioned retained section.
    #[derive(Serialize)]
    struct LegacySection {
        format: u32,
        recipe_id: String,
        recipe: PlayerInventoryRecipeV1,
        baseline: InventoryInitialTotalsV1,
        revision: u64,
        next_stack_id: u64,
        bags: Vec<PlayerBagV1>,
    }
    impl Graph for LegacySection {
        const LIMIT: usize = 1;
        fn walk(&self, b: &mut Budget) -> Result<()> {
            self.recipe_id.walk(b)?;
            self.recipe.walk(b)?;
            self.baseline.walk(b)?;
            self.bags.walk(b)
        }
    }
    #[test]
    fn v2_shared_decode_budget_and_retained_spare_capacity_are_charged() {
        let text = r#"{"format":2,"items":[],"inventories":[],"starter":[],"consumables":[{"item":"i","oxygen":1,"water":0,"food":0}]}"#;
        {
            let _scope = InventoryDecodeScope::enter();
            charge::<serde::de::value::Error>(MAX_INVENTORY_GRAPH_BYTES - 100).unwrap();
            assert!(serde_json::from_str::<PlayerInventoryRecipe>(text).is_err());
        }
        let mut versioned: PlayerInventoryRecipe = serde_json::from_str(text).unwrap();
        let base = graph(&versioned).unwrap();
        let PlayerInventoryRecipe::V2(v) = &mut versioned else {
            unreachable!()
        };
        v.consumables.reserve_exact(1023);
        let capacity = v.consumables.capacity();
        assert_eq!(
            graph(&versioned).unwrap() - base,
            (capacity - 4) * size_of::<PlayerConsumableV2>()
        );
        assert_eq!(size_of::<PlayerInventoryRecipeV2>(), 104);
    }
    #[test]
    fn versioned_recipe_layout_and_near_limit_v1_admission_match_legacy() {
        assert_eq!(
            size_of::<PlayerInventoryRecipe>(),
            size_of::<PlayerInventoryRecipeV1>()
        );
        assert_eq!(
            size_of::<PlayerInventorySnapshotV1>(),
            size_of::<LegacySection>()
        );
        let recipe = PlayerInventoryRecipeV1 {
            format: 1,
            items: vec![],
            inventories: vec![],
            starter: vec![],
        };
        let mut section = PlayerInventorySnapshotV1 {
            format: 1,
            recipe_id: "a".repeat(64),
            recipe: recipe.clone().into(),
            baseline: InventoryInitialTotalsV1 { items: vec![] },
            revision: 1,
            next_stack_id: 65537,
            bags: vec![],
        };
        // Fill a structural legacy graph to within one bag of its eight-copy
        // envelope. Semantic near-limit admission is covered in domain tests.
        for id in 1..=256 {
            let bag = PlayerBagV1 {
                owner: PlayerInventoryOwnerV1 {
                    player_id: id,
                    incarnation: 1,
                },
                revision: 1,
                consumed: vec![],
                inventories: vec![InventoryStateV1 {
                    key: "bag".into(),
                    overflow: vec![],
                    slots: (0..256)
                        .map(|slot| {
                            let mut item = String::with_capacity(96);
                            item.push_str(&"i".repeat(64));
                            Some(StackV1 {
                                id: StackId::new((id - 1) * 256 + slot + 1).unwrap(),
                                item,
                                count: 1,
                            })
                        })
                        .collect(),
                }],
            };
            section.bags.push(bag);
            if player_inventory_charge(&section, 8).is_err() {
                section.bags.pop();
                break;
            }
        }
        let legacy = LegacySection {
            format: section.format,
            recipe_id: section.recipe_id.clone(),
            recipe,
            baseline: section.baseline.clone(),
            revision: section.revision,
            next_stack_id: section.next_stack_id,
            bags: section.bags.clone(),
        };
        // Clone truncates string spare capacity: compare both the original
        // capacities and the reload capacities through the same legacy walk.
        let mut old_budget = Budget::default();
        old_budget.add(size_of::<LegacySection>()).unwrap();
        section.recipe_id.walk(&mut old_budget).unwrap();
        if let PlayerInventoryRecipe::V1(v) = &section.recipe {
            v.walk(&mut old_budget).unwrap();
        }
        section.baseline.walk(&mut old_budget).unwrap();
        section.bags.walk(&mut old_budget).unwrap();
        assert_eq!(graph(&section).unwrap(), old_budget.0);
        assert!(graph(&section).unwrap() * 8 > MAX_INVENTORY_GRAPH_BYTES * 99 / 100);
        player_inventory_charge(&section, 8).unwrap();
        let bytes = serde_json::to_vec(&legacy).unwrap();
        let decoded: PlayerInventorySnapshotV1 = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(serde_json::to_vec(&decoded).unwrap(), bytes);
        assert_eq!(graph(&decoded).unwrap(), graph(&legacy).unwrap());
        player_inventory_charge(&decoded, 8).unwrap();
        let carrier: PlayerInventorySnapshot = decoded.clone().into();
        assert_eq!(graph(&carrier).unwrap(), graph(&decoded).unwrap());
        assert_eq!(serde_json::to_vec(&carrier).unwrap(), bytes);
        let reloaded: PlayerInventorySnapshot = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(graph(&carrier).unwrap(), graph(&reloaded).unwrap());
        eprintln!(
            "V1 DTO/enum={} section={} near-limit-charge={} reloaded={} bags={}",
            size_of::<PlayerInventoryRecipe>(),
            size_of::<LegacySection>(),
            graph(&section).unwrap(),
            graph(&decoded).unwrap(),
            section.bags.len()
        );
    }
}

#[cfg(test)]
mod player_section_layout_tests {
    use super::*;
    #[test]
    fn player_section_carrier_layout_and_legacy_bytes_are_identical() {
        eprintln!(
            "layout section_v1={} carrier={} option_v1={} option_carrier={} section_v2={} bag_v1={} bag_v2={} item_total={}",
            size_of::<PlayerInventorySnapshotV1>(),
            size_of::<PlayerInventorySnapshot>(),
            size_of::<Option<PlayerInventorySnapshotV1>>(),
            size_of::<Option<PlayerInventorySnapshot>>(),
            size_of::<PlayerInventorySnapshotV2>(),
            size_of::<PlayerBagV1>(),
            size_of::<PlayerBagV2>(),
            size_of::<ItemTotalV1>()
        );
        assert_eq!(size_of::<PlayerBagV1>(), 72);
        assert_eq!(
            size_of::<PlayerInventorySnapshot>(),
            size_of::<PlayerInventorySnapshotV1>()
        );
        assert_eq!(
            size_of::<Option<PlayerInventorySnapshot>>(),
            size_of::<Option<PlayerInventorySnapshotV1>>()
        );
        for recipe in [
            PlayerInventoryRecipe::V1(PlayerInventoryRecipeV1 {
                format: 1,
                items: vec![],
                inventories: vec![],
                starter: vec![],
            }),
            PlayerInventoryRecipe::V2(Box::new(PlayerInventoryRecipeV2 {
                format: 2,
                items: vec![],
                inventories: vec![],
                starter: vec![],
                consumables: vec![],
            })),
        ] {
            let v = PlayerInventorySnapshotV1 {
                format: 1,
                recipe_id: "test".into(),
                recipe,
                baseline: InventoryInitialTotalsV1 { items: vec![] },
                revision: 1,
                next_stack_id: 1,
                bags: vec![],
            };
            let carrier: PlayerInventorySnapshot = v.clone().into();
            assert_eq!(graph(&v).unwrap(), graph(&carrier).unwrap());
            assert_eq!(
                serde_json::to_vec(&v).unwrap(),
                serde_json::to_vec(&carrier).unwrap()
            );
            let decoded: PlayerInventorySnapshot =
                serde_json::from_slice(&serde_json::to_vec(&v).unwrap()).unwrap();
            assert_eq!(carrier, decoded);
        }
    }
}

#[cfg(test)]
mod player_section_budget_tests {
    use super::*;
    #[test]
    fn boxed_root_sparse_spare_capacity_and_nested_decode_share_budget() {
        let text = r#"{"bags":[{"sent":[],"received":[{"item":"i","count":1}],"owner":{"player_id":1,"incarnation":1},"revision":1,"consumed":[],"inventories":[]}],"format":2,"recipe_id":"id","recipe":{"format":1,"items":[],"inventories":[],"starter":[]},"baseline":{"items":[]},"revision":1,"next_stack_id":1}"#;
        let mut value: PlayerInventorySnapshot = serde_json::from_str(text).unwrap();
        let base = graph(&value).unwrap();
        let PlayerInventorySnapshot::V2(v) = &value else {
            unreachable!()
        };
        assert_eq!(
            base,
            graph(v.as_ref()).unwrap() + size_of::<PlayerInventorySnapshot>()
        );
        let PlayerInventorySnapshot::V2(v) = &mut value else {
            unreachable!()
        };
        v.bags[0].sent.reserve_exact(1024);
        let cap = v.bags[0].sent.capacity();
        assert_eq!(
            graph(&value).unwrap() - base,
            cap * size_of::<ItemTotalV1>()
        );
        {
            let _scope = InventoryDecodeScope::enter();
            charge::<serde::de::value::Error>(MAX_INVENTORY_GRAPH_BYTES - 100).unwrap();
            assert!(serde_json::from_str::<PlayerInventorySnapshot>(text).is_err());
        }
        assert!(serde_json::from_str::<PlayerInventorySnapshot>(text).is_ok());
    }
}

#[cfg(test)]
mod transfer_boundary_tests {
    use super::*;

    #[test]
    fn boundary_v2_encoded_ceiling_is_dominated_by_graph_charge() {
        // Induction over the section's JSON tree: each local object shell,
        // including maximal decimal scalars and one array separator, fits in
        // twice its charged Rust storage. Empty strings/vectors here stand for
        // child payloads, not valid API input. Keys contain only unescaped
        // ASCII, so adding their bytes costs <= twice the charged capacity.
        // Each vector reserves >= len * size_of(element), also after reload.
        fn shell<T: Serialize>(value: &T) {
            let bytes = serde_json::to_vec(value).unwrap().len() + 1;
            assert!(
                bytes <= 2 * size_of::<T>(),
                "{}: {bytes} > {}",
                std::any::type_name::<T>(),
                2 * size_of::<T>()
            );
        }
        let stack = || StackV1 {
            id: StackId::new(u64::MAX).unwrap(),
            item: String::new(),
            count: u32::MAX,
        };
        shell(&String::new());
        shell(&ItemDefinitionV1 {
            key: String::new(),
            stack_limit: u32::MAX,
        });
        shell(&SlotRuleV1 {
            allowed_items: vec![],
        });
        shell(&InventoryDefinitionV1 {
            key: String::new(),
            initial_slots: u16::MAX,
            slot_rules: vec![],
            force_add_when_zero: false,
            overflow_limit: u16::MAX,
        });
        shell(&Some(stack()));
        shell(&Option::<StackV1>::None);
        shell(&DeferredStackV1 {
            stack: stack(),
            requested_slot: u16::MAX,
        });
        shell(&InventoryStateV1 {
            key: String::new(),
            slots: vec![],
            overflow: vec![],
        });
        shell(&ItemTotalV1 {
            item: String::new(),
            count: u64::MAX,
        });
        shell(&PlayerStarterItemV1 {
            inventory: String::new(),
            requested_slot: u16::MAX,
            item: String::new(),
            count: u32::MAX,
        });
        shell(&PlayerConsumableV2 {
            item: String::new(),
            oxygen: u32::MAX,
            water: u32::MAX,
            food: u32::MAX,
        });
        shell(&PlayerBagV2 {
            owner: PlayerInventoryOwnerV1 {
                player_id: u64::MAX,
                incarnation: u64::MAX,
            },
            revision: u64::MAX,
            consumed: vec![],
            received: vec![],
            sent: vec![],
            inventories: vec![],
        });
        let v1 = PlayerInventoryRecipeV1 {
            format: u32::MAX,
            items: vec![],
            inventories: vec![],
            starter: vec![],
        };
        shell(&v1);
        let v2 = PlayerInventoryRecipeV2 {
            format: u32::MAX,
            items: vec![],
            inventories: vec![],
            starter: vec![],
            consumables: vec![],
        };
        shell(&v2);
        for recipe in [v1.into(), v2.into()] {
            let section = PlayerInventorySnapshotV2 {
                format: u32::MAX,
                recipe_id: String::new(),
                recipe,
                baseline: InventoryInitialTotalsV1 { items: vec![] },
                revision: u64::MAX,
                next_stack_id: u64::MAX,
                bags: vec![],
            };
            // Includes the inline recipe and baseline roots in the section
            // shell; V2's additional boxed recipe root only increases charge.
            shell(&section);
        }
        let pins_min = 128
            * (size_of::<InventoryInitialTotalsV1>()
                + 64
                + crate::native_player_runtime::inventory_pin_v2_overhead_bytes());
        let scratch = MAX_INVENTORY_ITEMS
            * (4 * size_of::<u128>() + 3 * (size_of::<ItemTotalV1>() + 64))
            + 2 * 1024
            + 64 * 1024;
        let max_graph = (MAX_INVENTORY_GRAPH_BYTES - pins_min - scratch) / 8;
        let max_encoded = 2 * max_graph;
        assert_eq!(pins_min, 37_888);
        assert_eq!(scratch, 428_032);
        assert_eq!(max_graph, 8_330_368);
        assert_eq!(max_encoded, 16_660_736);
        assert!(max_encoded < MAX_INVENTORY_STATE_BYTES);
        eprintln!(
            "V2 encoded upper bound={max_encoded} < {} (gap={}), G<={max_graph}, pins>={pins_min}, scratch={scratch}",
            MAX_INVENTORY_STATE_BYTES,
            MAX_INVENTORY_STATE_BYTES - max_encoded
        );
    }

    #[test]
    fn boundary_encoded_helper_accepts_exact_limit_and_refuses_next_byte() {
        // Helper coverage only: this single string is not an inventory section
        // and is unreachable through V2's stricter graph admission.
        let mut payload = "a".repeat(MAX_INVENTORY_STATE_BYTES - 2);
        assert_eq!(
            encoded(&payload, MAX_INVENTORY_STATE_BYTES).unwrap(),
            MAX_INVENTORY_STATE_BYTES
        );
        payload.push('a');
        assert_eq!(
            encoded(&payload, MAX_INVENTORY_STATE_BYTES)
                .unwrap_err()
                .to_string(),
            "inventory encoding limit"
        );
    }
}

#[cfg(test)]
mod transfer_graph_boundary_tests {
    use super::*;
    use crate::{
        config::ServerConfig,
        native_player_inventory::{prepare_transfer_offline, transfer_staging_tests as fixtures},
        native_player_runtime::{CheckpointWriter, LeavePhase, PlayerRegistry, activate_player},
        persistence::SnapshotStore,
        simulation::{WorldSnapshot, build_app, capture},
    };
    use std::time::{Duration, Instant};

    fn section(s: &WorldSnapshot) -> &PlayerInventorySnapshotV2 {
        s.player_inventory.as_ref().unwrap().as_v2().unwrap()
    }
    fn section_mut(s: &mut WorldSnapshot) -> &mut PlayerInventorySnapshotV2 {
        s.player_inventory.as_mut().unwrap().as_v2_mut().unwrap()
    }
    fn charge(s: &WorldSnapshot) -> usize {
        let s = section(s);
        8 * player_inventory_v2_bytes(s).unwrap()
            + 128
                * (graph(&s.baseline).unwrap()
                    + 64
                    + crate::native_player_runtime::inventory_pin_v2_overhead_bytes())
            + MAX_INVENTORY_ITEMS * (4 * size_of::<u128>() + 3 * (size_of::<ItemTotalV1>() + 64))
            + 2 * 1024
            + 64 * 1024
    }
    fn add_balanced_row(bag: &mut PlayerBagV2, item: &str) {
        for rows in [&mut bag.received, &mut bag.sent] {
            let i = rows
                .binary_search_by(|v| v.item.as_str().cmp(item))
                .unwrap_err();
            rows.insert(
                i,
                ItemTotalV1 {
                    item: item.into(),
                    count: 9,
                },
            );
        }
    }
    fn fixture() -> WorldSnapshot {
        let item = "a".repeat(64);
        let extra: Vec<String> = (0..24).map(|n| format!("z{n:063}")).collect();
        let definition = |key: &str, slots| InventoryDefinitionV1 {
            key: key.into(),
            initial_slots: slots,
            slot_rules: vec![
                SlotRuleV1 {
                    allowed_items: vec![item.clone()]
                };
                slots as usize
            ],
            force_add_when_zero: false,
            overflow_limit: 0,
        };
        let recipe = PlayerInventoryRecipeV1 {
            format: 1,
            items: std::iter::once(item.clone())
                .chain(extra.clone())
                .map(|key| ItemDefinitionV1 {
                    key,
                    stack_limit: 2,
                })
                .collect(),
            inventories: vec![definition("bag", 256), definition("spare", 1)],
            starter: (0..256)
                .map(|slot| PlayerStarterItemV1 {
                    inventory: "bag".into(),
                    requested_slot: slot,
                    item: item.clone(),
                    count: 2,
                })
                .collect(),
        };
        let mut s = fixtures::fixture(recipe, 256);
        for bag in &mut section_mut(&mut s).bags {
            bag.received = extra[..16]
                .iter()
                .map(|item| ItemTotalV1 {
                    item: item.clone(),
                    count: 9,
                })
                .collect();
            bag.sent = bag.received.clone();
        }
        s
    }

    #[test]
    fn boundary_v2_graph_charge_transfer_growth_reload_and_returning_leave() {
        let mut s = fixture();
        let item = section(&s).recipe.items()[0].key.clone();
        let base_charge = charge(&s);
        eprintln!(
            "V2 initial charge={base_charge} graph={} baseline={} bags={}",
            player_inventory_v2_bytes(section(&s)).unwrap(),
            graph(&section(&s).baseline).unwrap(),
            section(&s).bags.len()
        );
        s.validate().unwrap();
        // Add genuine balanced cumulative flow rows to non-endpoints until the
        // next row pair would cross admission. No string spare-capacity padding.
        let padding: Vec<_> = section(&s).recipe.items()[17..]
            .iter()
            .map(|i| i.key.clone())
            .chain(std::iter::once(item.clone()))
            .collect();
        let mut refused_at = None;
        let mut current_charge = base_charge;
        'padding: for i in 2..section(&s).bags.len() {
            for key in &padding {
                let bag = &mut section_mut(&mut s).bags[i];
                let before = graph(bag).unwrap();
                add_balanced_row(bag, key);
                current_charge += 8 * (graph(bag).unwrap() - before);
                if current_charge > MAX_INVENTORY_GRAPH_BYTES {
                    refused_at = Some((i, key.clone()));
                    break 'padding;
                }
            }
        }
        let (i, refused_key) = refused_at.expect("fixture must bracket the unchanged graph cap");
        let rejected = s.clone();
        let bag = &mut section_mut(&mut s).bags[i];
        for rows in [&mut bag.received, &mut bag.sent] {
            let j = rows.binary_search_by(|v| v.item.cmp(&refused_key)).unwrap();
            rows.remove(j);
            rows.shrink_to_fit();
        }
        let last_accepted = s;
        let accepted_charge = charge(&last_accepted);
        let refused_charge = charge(&rejected);
        assert_eq!(refused_charge, current_charge);
        assert!(accepted_charge <= MAX_INVENTORY_GRAPH_BYTES);
        assert!(refused_charge > MAX_INVENTORY_GRAPH_BYTES);
        assert!(accepted_charge > MAX_INVENTORY_GRAPH_BYTES * 99 / 100);
        assert_eq!(
            rejected.validate().unwrap_err().to_string(),
            "player inventory allocation limit"
        );
        last_accepted.validate().unwrap();
        // The first refused pair is balanced and valid except for the budget:
        // removing exactly that row pair restores all semantic checks.
        let request = fixtures::request(&last_accepted);
        let mut request = PlayerInventoryTransferRequestV1 {
            amount: 2,
            ..request
        };
        let before = serde_json::to_vec(&last_accepted).unwrap();
        let err = prepare_transfer_offline(&last_accepted, &request).unwrap_err();
        assert_eq!(err.to_string(), "player inventory allocation limit");
        assert_eq!(serde_json::to_vec(&last_accepted).unwrap(), before);
        let mut refused_live = build_app(&ServerConfig::default(), &last_accepted).unwrap();
        for (id, epoch) in [(1, 1001), (2, 1002)] {
            refused_live
                .world_mut()
                .resource_mut::<PlayerRegistry>()
                .reserve(fixtures::key(id), epoch, fixtures::checkpoint())
                .unwrap();
            activate_player(refused_live.world_mut(), epoch).unwrap();
        }
        let live_before = capture(refused_live.world_mut()).unwrap();
        let err = crate::native_player_runtime::apply_player_inventory_transfer(
            refused_live.world_mut(),
            fixtures::key(1),
            1001,
            fixtures::key(2),
            1002,
            &request,
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "player inventory allocation limit");
        assert_eq!(capture(refused_live.world_mut()).unwrap(), live_before);
        refused_live
            .world_mut()
            .resource_mut::<PlayerRegistry>()
            .reserve(fixtures::key(257), 2000, fixtures::checkpoint())
            .unwrap();
        let pending = capture(refused_live.world_mut()).unwrap();
        assert!(activate_player(refused_live.world_mut(), 2000).is_err());
        assert_eq!(capture(refused_live.world_mut()).unwrap(), pending);
        drop(refused_live);
        // Removing one non-endpoint's padding makes room for the endpoint key
        // insertions (16 -> 17 rows, decoder capacity 16 -> 32 at each end).
        let mut room = last_accepted.clone();
        let bag = section_mut(&mut room)
            .bags
            .iter_mut()
            .find(|b| b.owner.player_id > 2 && b.received.len() > 16)
            .unwrap();
        for rows in [&mut bag.received, &mut bag.sent] {
            rows.retain(|v| !padding.contains(&v.item));
        }
        room = serde_json::from_slice(&serde_json::to_vec(&room).unwrap()).unwrap();
        room.validate().unwrap();
        request.source_cas.expected_player_revision = room
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(1)
            .unwrap()
            .revision;
        let (transferred, _) = prepare_transfer_offline(&room, &request).unwrap();
        let after_charge = charge(&transferred);
        assert!(after_charge <= MAX_INVENTORY_GRAPH_BYTES);
        eprintln!(
            "V2 bracket cap={} accepted={accepted_charge} refused={refused_charge}, transfer input={} output={after_charge}",
            MAX_INVENTORY_GRAPH_BYTES,
            charge(&room)
        );
        assert_eq!(
            section(&transferred).next_stack_id,
            section(&room).next_stack_id
        );
        assert_eq!(section(&transferred).bag(1).unwrap().sent[0].count, 2);
        assert_eq!(section(&transferred).bag(2).unwrap().received[0].count, 2);
        let input_graph = player_inventory_v2_bytes(section(&room)).unwrap();
        let bytes = serde_json::to_vec(&transferred).unwrap();
        let decoded: WorldSnapshot = serde_json::from_slice(&bytes).unwrap();
        decoded.validate().unwrap();
        let reload_graph = player_inventory_v2_bytes(section(&decoded)).unwrap();
        assert_eq!(charge(&decoded), after_charge);
        assert_eq!(decoded, transferred);
        // Real stopped-store publication/reload, then returning activation and
        // a captured leave pin; neither path may issue starter state again.
        let dir = tempfile::tempdir().unwrap();
        let mut store = SnapshotStore::open(dir.path()).unwrap();
        store.save(&decoded).unwrap();
        let disk = store.load().unwrap().unwrap().snapshot;
        assert_eq!(disk, decoded);
        let mut app = build_app(&ServerConfig::default(), &disk).unwrap();
        let registry_before = capture(app.world_mut()).unwrap();
        let context = disk.native_players.as_ref().unwrap().context().clone();
        assert_eq!(
            context,
            registry_before
                .native_players
                .as_ref()
                .unwrap()
                .context()
                .clone()
        );
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .reserve(fixtures::key(1), 1001, fixtures::checkpoint())
            .unwrap();
        activate_player(app.world_mut(), 1001).unwrap();
        let active = capture(app.world_mut()).unwrap();
        assert_eq!(active.player_inventory, disk.player_inventory);
        assert_eq!(active.native_players, disk.native_players);
        let revision = active
            .native_players
            .as_ref()
            .unwrap()
            .projection_by_id(1)
            .unwrap()
            .revision;
        app.world_mut()
            .resource_mut::<PlayerRegistry>()
            .request_leave(
                fixtures::key(1),
                1001,
                revision,
                Instant::now(),
                Duration::from_secs(10),
            )
            .unwrap();
        assert_eq!(capture(app.world_mut()).unwrap(), active);
        let mut writer = CheckpointWriter::spawn(store).unwrap();
        writer.submit(app.world_mut()).unwrap();
        writer
            .finish_before(app.world_mut(), Instant::now() + Duration::from_secs(10))
            .unwrap();
        assert!(matches!(
            app.world()
                .resource::<PlayerRegistry>()
                .leave_status(fixtures::key(1), 1001)
                .unwrap()
                .phase,
            LeavePhase::Durable { .. }
        ));
        assert_eq!(capture(app.world_mut()).unwrap(), active);
        eprintln!(
            "V2 cap={} accepted={accepted_charge} refused={refused_charge} transfer-input={} transfer-output={after_charge} graph-input={input_graph} graph-reload={reload_graph} encoded={} bags={} allocator={}",
            MAX_INVENTORY_GRAPH_BYTES,
            charge(&room),
            encoded(section(&decoded), MAX_INVENTORY_STATE_BYTES).unwrap(),
            section(&decoded).bags.len(),
            section(&decoded).next_stack_id
        );
    }
}
