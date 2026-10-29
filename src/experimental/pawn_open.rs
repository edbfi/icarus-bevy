//! The joining player's pawn open (`F-797`), on [`super::actor_open`].
//!
//! The original opens the player's pawn on its owner's connection with
//! `RemoteRole = ROLE_AutonomousProxy`, `Owner` and `Controller` naming the
//! PlayerController, `Instigator` itself and `PlayerState` the player's
//! PlayerState; three properties reference components (`StatContainer`,
//! `ActorState`, `InventoryComponent`); every component gets a block, and
//! the runtime inventories follow as class-named subobjects.
//!
//! [`PawnContent`] carries what the session supplies beyond identifiers:
//! the actor's `IcarusUID`, gameplay tags and frozen-movement flag, the
//! character state's values, the stat array and the inventories with empty
//! slots, in the shapes the capture decodes to. Stat, row and tag indices
//! are the caller's; nothing here reads original files or gameplay tables.
use std::collections::BTreeMap;

use anyhow::{Result, ensure};

use super::actor_open::{
    Block, BlockContent, BlockProperties, ComponentActorOpen, CustomField, OpenComponent,
    OpenSubobject, Reference, compose_component_actor_open,
};
use super::archive::ByteOrder;
use super::control_messages::ArchiveText;
use super::packet::{BitWriter, Payload};
use super::rep_layout::{
    GameplayTag, NetworkName, PropertyStream, RepArray, RepCommand, RepEntry, RepLayoutCodec,
    RepLayoutLimits, RepValue, StatPairRepState, encode_rep_properties,
};
use super::rep_rpc::{
    CustomDeltaHeader, FAST_ARRAY_COUNT_CAP, FastArrayDelta, FastArrayItem, FastArrayItemLayout,
    STRUCT_DELTA_NET_VERSION, write_custom_delta_header, write_fast_array_delta,
};
use super::session_open::{
    ActorKey, ComposedActorOpen, ConnectionExports, NetGuidCache, PathSegment, actor_codec, entry,
};

/// Handles of the `BP_IcarusPlayerCharacterSurvival_C` layout the pawn open
/// writes (F-797).
pub mod handle {
    /// `AActor::RemoteRole`.
    pub const REMOTE_ROLE: u32 = 5;
    /// `AActor::Owner`.
    pub const OWNER: u32 = 13;
    /// `AActor::Role`.
    pub const ROLE: u32 = 14;
    /// `AActor::Instigator`.
    pub const INSTIGATOR: u32 = 15;
    /// `APawn::PlayerState`.
    pub const PLAYER_STATE: u32 = 17;
    /// `APawn::Controller`.
    pub const CONTROLLER: u32 = 18;
    /// `AIcarusCharacter::IcarusUID` (`FIntProperty`).
    pub const ICARUS_UID: u32 = 46;
    /// `AIcarusCharacter::StatContainer`.
    pub const STAT_CONTAINER: u32 = 47;
    /// `AIcarusCharacter::ActorState`.
    pub const ACTOR_STATE: u32 = 48;
    /// `AIcarusCharacter::GameplayTags`.
    pub const GAMEPLAY_TAGS: u32 = 50;
    /// `AIcarusPlayerCharacter::InventoryComponent`.
    pub const INVENTORY_COMPONENT: u32 = 56;
    /// `AIcarusPlayerCharacter::ReplicatedControlRotation` (`FRotator`).
    pub const REPLICATED_CONTROL_ROTATION: u32 = 58;
    /// `AIcarusPlayerCharacter::RespawnCount` (`FIntProperty`).
    pub const RESPAWN_COUNT: u32 = 63;
    /// `AIcarusPlayerCharacter::HasGrantedLoadout` (`FBoolProperty`).
    pub const HAS_GRANTED_LOADOUT: u32 = 64;
    /// `AIcarusPlayerCharacter::CosmeticArmourOverrides` (item-static row
    /// indices).
    pub const COSMETIC_ARMOUR_OVERRIDES: u32 = 66;
    /// `bServerFrozenMovement` (`FBoolProperty`).
    pub const SERVER_FROZEN_MOVEMENT: u32 = 67;
}

/// Handles of the `UPlayerCharacterState` layout (`player_character_state_rep_layout`)
/// that [`CharacterVitals`] and a saved character's restore write.
pub mod character_state_handle {
    /// `UActorState::Health` and `MaxHealth`.
    pub const HEALTH: u32 = 3;
    pub const MAX_HEALTH: u32 = 4;
    /// `UActorState::CurrentBiome` (1-byte row index).
    pub const CURRENT_BIOME: u32 = 11;
    /// `UActorState::CurrentAliveState` (`EAliveState`, 1 bit).
    pub const CURRENT_ALIVE_STATE: u32 = 12;
    /// `UCharacterState::Stamina` and `MaxStamina`.
    pub const STAMINA: u32 = 13;
    pub const MAX_STAMINA: u32 = 14;
    /// `UCharacterState::TotalExperience`.
    pub const TOTAL_EXPERIENCE: u32 = 15;
    /// `UCharacterState::GrowthRowHandle` (1-byte row index).
    pub const GROWTH_ROW: u32 = 16;
    /// `USurvivalCharacterState` levels and maxima.
    pub const OXYGEN: u32 = 18;
    pub const WATER: u32 = 19;
    pub const FOOD: u32 = 20;
    pub const MIN_OXYGEN: u32 = 21;
    pub const MAX_OXYGEN: u32 = 22;
    pub const MIN_WATER: u32 = 23;
    pub const MAX_WATER: u32 = 24;
    pub const MIN_FOOD: u32 = 25;
    pub const MAX_FOOD: u32 = 26;
    pub const RADIATION: u32 = 28;
    pub const MIN_RADIATION: u32 = 29;
    pub const MAX_RADIATION: u32 = 30;
    /// The last handle of the layout (`AIDetectionPercentage`).
    pub const LAST: u32 = 32;
}

/// `UIcarusStatContainer::ReplicatedStatArray`'s field index: the first
/// field after `UActorComponent`'s two (F-705, F-708).
pub const STAT_ARRAY_FIELD_INDEX: u32 = 2;
/// `UInventory::Slots`' field index: after `UActorComponent`'s two,
/// `UTraitBehaviour::OwningComponent` and `CurrentWeight` (F-708).
pub const INVENTORY_SLOTS_FIELD_INDEX: u32 = 4;

/// `UInventory` handles the open writes (`inventory_rep_layout`).
const INVENTORY_IS_ACTIVE: u32 = 2;
const INVENTORY_OWNING_COMPONENT: u32 = 3;
pub(crate) const INVENTORY_CURRENT_WEIGHT: u32 = 4;
const INVENTORY_INFO_ROW: u32 = 7;

/// `FInventorySlot` handles a slot writes (`inventory_slot_commands`): the
/// item's static row (2), its flag (10), string (11), integer (12) and tags
/// (13), then `Query` (14), `Locked` (15), `LastItem` (16), `Slotable` (17)
/// and `Index` (18). An item slot adds its nonempty dynamic data (3) and
/// stats (7); the other item arrays (1, 4..=6, 8, 9) stay unsent (F-866).
pub(crate) mod slot_handle {
    pub const ITEM_ROW: u32 = 2;
    /// `ItemData.ItemDynamicData`: (`EDynamicItemProperties`, value) pairs.
    pub const ITEM_DYNAMIC: u32 = 3;
    /// `ItemData.CustomProperties.Stats`: (stat name, value) pairs.
    pub const ITEM_STATS: u32 = 7;
    /// `ItemData.CustomProperties.Alterations`: alteration names.
    pub const ITEM_ALTERATIONS: u32 = 8;
    /// `ItemData.bIsItemInstance`.
    pub const ITEM_FLAG: u32 = 10;
    pub const ITEM_STRING: u32 = 11;
    pub const ITEM_INTEGER: u32 = 12;
    pub const ITEM_TAGS: u32 = 13;
    pub const QUERY: u32 = 14;
    pub const LOCKED: u32 = 15;
    pub const LAST_ITEM: u32 = 16;
    pub const SLOTABLE: u32 = 17;
    pub const INDEX: u32 = 18;
}

/// A 2-byte row handle naming no row.
const NO_ROW: u16 = u16::MAX;

/// The pawn's channel: the next index after the PlayerController's, as
/// captured (F-797).
pub const PAWN_CHANNEL_INDEX: u32 = 3;

/// `ROLE_AutonomousProxy` for the owning connection and `ROLE_Authority`,
/// as captured (F-797).
const OWNER_REMOTE_ROLE: u8 = 2;
const ROLE: u8 = 3;

/// A pawn component: its name, class network checksum (F-776) and the
/// `UActorComponent` bools its block carries at open.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PawnComponent {
    pub name: String,
    pub checksum: u32,
    /// `[handle, value]` pairs of handles `1..=2`, strictly ascending.
    pub rep_bools: Vec<(u32, bool)>,
}

/// Release identifiers and layouts for the pawn's content blocks.
#[derive(Clone, Debug, PartialEq)]
pub struct PawnContentPlan {
    /// `UActorComponent`'s layout, for component bools.
    pub component_layout: Vec<RepCommand>,
    /// `UPlayerCharacterState`'s layout.
    pub character_state_layout: Vec<RepCommand>,
    /// The runtime inventories' class chain (`Inventory`, then its package).
    pub inventory_class: Vec<PathSegment>,
    /// `UInventory`'s layout.
    pub inventory_layout: Vec<RepCommand>,
    /// `FInventorySlot`'s element layout.
    pub slot_layout: Vec<RepCommand>,
    /// `FClassNetCache` field-index bounds of `UIcarusStatContainer` and
    /// `UInventory` (F-708).
    pub stat_container_bound: u32,
    pub inventory_bound: u32,
}

/// Everything a session needs to open a player's pawn.
#[derive(Clone, Debug)]
pub struct PawnOpenPlan {
    /// `Default__<Class>_C`, then its package.
    pub archetype: Vec<PathSegment>,
    /// `PersistentLevel`, the world, then the map package.
    pub level: Vec<PathSegment>,
    /// The components, in block order.
    pub components: Vec<PawnComponent>,
    /// Indices into `components` of `StatContainer`, `ActorState` and
    /// `InventoryComponent` (handles 47, 48, 56).
    pub stat_container: usize,
    pub actor_state: usize,
    pub inventory_component: usize,
    /// The `BP_IcarusPlayerCharacterSurvival_C` layout.
    pub layout: Vec<RepCommand>,
    pub content: PawnContentPlan,
    pub engine_net_version: u32,
}

/// The character state values a fresh character carries at open, in the
/// handles the capture shows (F-797); every field is sent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CharacterVitals {
    pub health: i32,
    pub max_health: i32,
    pub stamina: i32,
    pub max_stamina: i32,
    pub oxygen: i32,
    pub max_oxygen: i32,
    pub water: i32,
    pub max_water: i32,
    pub food: i32,
    pub max_food: i32,
    /// Row indices of the biome and growth tables.
    pub biome_row: u8,
    pub growth_row: u8,
}

impl CharacterVitals {
    /// The values by `UPlayerCharacterState` handle.
    pub fn values(&self) -> BTreeMap<u32, RepValue> {
        use character_state_handle as h;
        BTreeMap::from([
            (h::HEALTH, RepValue::Int(self.health)),
            (h::MAX_HEALTH, RepValue::Int(self.max_health)),
            (h::CURRENT_BIOME, RepValue::Byte(self.biome_row)),
            (h::STAMINA, RepValue::Int(self.stamina)),
            (h::MAX_STAMINA, RepValue::Int(self.max_stamina)),
            (h::GROWTH_ROW, RepValue::Byte(self.growth_row)),
            (h::OXYGEN, RepValue::Int(self.oxygen)),
            (h::WATER, RepValue::Int(self.water)),
            (h::FOOD, RepValue::Int(self.food)),
            (h::MAX_OXYGEN, RepValue::Int(self.max_oxygen)),
            (h::MAX_WATER, RepValue::Int(self.max_water)),
            (h::MAX_FOOD, RepValue::Int(self.max_food)),
        ])
    }
}

/// A fast array's items and the server's replication key for it (the
/// count of changes it has marked; the base key at open is 0).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FastArrayContent<T> {
    pub replication_key: i32,
    /// Items in ID order; IDs run from 1.
    pub items: Vec<T>,
}

impl<T> Default for FastArrayContent<T> {
    fn default() -> Self {
        Self {
            replication_key: 0,
            items: Vec::new(),
        }
    }
}

/// An item in a slot as `FItemData` replicates it (F-866): its
/// `D_ItemsStatic` row, its `ItemDynamicData` (`EDynamicItemProperties`
/// value, value) pairs and its `CustomProperties.Stats` (stat name, value)
/// pairs; the other fields keep their defaults.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SlotItem {
    pub row: u16,
    pub dynamic: Vec<(u32, i32)>,
    pub stats: Vec<(ArchiveText, i32)>,
    /// `CustomProperties.Alterations` names.
    pub alterations: Vec<ArchiveText>,
    /// `DatabaseGUID`; empty by default.
    pub guid: ArchiveText,
    /// `ItemOwnerLookupId`; -1 by default.
    pub owner_lookup_id: i32,
}

impl SlotItem {
    /// An item of `row` with no dynamic data, stats or alterations and the
    /// default GUID and owner lookup ID.
    pub fn new(row: u16) -> Result<Self> {
        Ok(Self {
            row,
            dynamic: Vec::new(),
            stats: Vec::new(),
            alterations: Vec::new(),
            guid: ArchiveText::new(&[])?,
            owner_lookup_id: -1,
        })
    }
}

/// An inventory slot: its query row and index, and its item if any.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventorySlot {
    pub query_row: u16,
    pub index: i32,
    pub item: Option<SlotItem>,
}

impl InventorySlot {
    /// An empty slot.
    pub fn empty(query_row: u16, index: i32) -> Self {
        Self {
            query_row,
            index,
            item: None,
        }
    }
}

/// One runtime inventory: its info row, optional caller-supplied weight and slots.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct PawnInventory {
    pub info_row: u8,
    /// Explicit `CurrentWeight` at rep-layout handle 4 (F-708). `None` omits
    /// the property; no weight is inferred from slots, saves or tables.
    pub current_weight: Option<i32>,
    pub slots: FastArrayContent<InventorySlot>,
}

/// One player's pawn content beyond identifiers; [`Default`] adds nothing
/// (every component opens without properties, no inventories).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PawnContent {
    /// `IcarusUID` (handle 46).
    pub icarus_uid: Option<i32>,
    /// `GameplayTags` (handle 50) as net indices of the release's tag
    /// dictionary; empty sends none.
    pub gameplay_tags: Vec<u16>,
    /// `bServerFrozenMovement` (handle 67), sent when set.
    pub frozen_movement: bool,
    /// `UPlayerCharacterState` values by handle (`3..=32`).
    pub character_state: BTreeMap<u32, RepValue>,
    /// `ReplicatedStatArray`; `None` sends no field.
    pub stats: Option<FastArrayContent<StatPairRepState>>,
    pub inventories: Vec<PawnInventory>,
}

/// One player's pawn values.
#[derive(Clone, Debug, PartialEq)]
pub struct PawnOpenState {
    /// The pawn's key.
    pub key: ActorKey,
    /// The owning PlayerController's and the PlayerState's GUIDs.
    pub controller: u32,
    pub player_state: u32,
    /// The spawn location.
    pub location: [f32; 3],
    pub content: PawnContent,
}

/// A custom-delta fast-array field with every item new (base key 0, no
/// deletes, IDs from 1), as the open sends it.
fn fast_array_field(
    index: u32,
    replication_key: i32,
    items: Vec<PropertyStream>,
    element: &[RepCommand],
    engine_net_version: u32,
    order: ByteOrder,
) -> Result<CustomField> {
    let delta = FastArrayDelta {
        array_replication_key: replication_key,
        base_replication_key: 0,
        deleted_ids: Vec::new(),
        changed: items
            .into_iter()
            .zip(1..)
            .map(|(stream, id)| (id, FastArrayItem::Delta(stream)))
            .collect(),
    };
    fast_array_delta_field(index, &delta, element, engine_net_version, order)
}

/// A custom-delta fast-array field carrying `delta` (items as property
/// streams over `element`), as an open or a later update sends it.
pub fn fast_array_delta_field(
    index: u32,
    delta: &FastArrayDelta,
    element: &[RepCommand],
    engine_net_version: u32,
    order: ByteOrder,
) -> Result<CustomField> {
    let codec = RepLayoutCodec {
        actor: actor_codec(engine_net_version, order),
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let mut writer = BitWriter::new();
    write_custom_delta_header(
        &mut writer,
        &CustomDeltaHeader {
            struct_delta: (engine_net_version >= STRUCT_DELTA_NET_VERSION).then_some(true),
            array_index: None,
        },
        engine_net_version,
        1,
    )?;
    write_fast_array_delta(
        &mut writer,
        delta,
        &FastArrayItemLayout::Delta(element.to_vec()),
        &codec,
        FAST_ARRAY_COUNT_CAP,
    )?;
    Ok(CustomField {
        index,
        payload: Payload::from_writer(writer),
    })
}

fn item_stream(entries: Vec<(u32, RepValue)>) -> PropertyStream {
    PropertyStream {
        entries: entries
            .into_iter()
            .map(|(handle, value)| entry(handle, value))
            .collect(),
        terminator_checksum: None,
    }
}

/// A `(element field, value)` pair array, two fields per element.
fn pair_array(pairs: impl ExactSizeIterator<Item = (RepValue, RepValue)>) -> Result<RepValue> {
    let count = u16::try_from(pairs.len())?;
    let entries = pairs
        .zip(0u32..)
        .flat_map(|((first, second), element)| {
            [
                entry(2 * element + 1, first),
                entry(2 * element + 2, second),
            ]
        })
        .collect();
    Ok(RepValue::DynamicArray(RepArray {
        count,
        stream: PropertyStream {
            entries,
            terminator_checksum: None,
        },
    }))
}

/// A slot's every field, as an initial open sends it: an item's row, its
/// nonempty dynamic data and stats and `bIsItemInstance`, or the empty
/// slot's defaults, then the slot's own fields (`LastItem` is the item's
/// row).
pub fn slot_stream(slot: &InventorySlot) -> Result<PropertyStream> {
    use slot_handle as h;
    let row = slot.item.as_ref().map_or(NO_ROW, |item| item.row);
    let mut entries = vec![(h::ITEM_ROW, RepValue::UInt16(row))];
    if let Some(item) = &slot.item {
        if !item.dynamic.is_empty() {
            entries.push((
                h::ITEM_DYNAMIC,
                pair_array(
                    item.dynamic.iter().map(|(property, value)| {
                        (RepValue::Enum(*property), RepValue::Int(*value))
                    }),
                )?,
            ));
        }
        if !item.stats.is_empty() {
            entries.push((
                h::ITEM_STATS,
                pair_array(item.stats.iter().map(|(name, value)| {
                    (
                        RepValue::NetworkName(NetworkName::String {
                            text: name.clone(),
                            number: 0,
                        }),
                        RepValue::Int(*value),
                    )
                }))?,
            ));
        }
        if !item.alterations.is_empty() {
            let count = u16::try_from(item.alterations.len())?;
            entries.push((
                h::ITEM_ALTERATIONS,
                RepValue::DynamicArray(RepArray {
                    count,
                    stream: PropertyStream {
                        entries: item
                            .alterations
                            .iter()
                            .zip(1..)
                            .map(|(name, handle)| {
                                entry(
                                    handle,
                                    RepValue::NetworkName(NetworkName::String {
                                        text: name.clone(),
                                        number: 0,
                                    }),
                                )
                            })
                            .collect(),
                        terminator_checksum: None,
                    },
                }),
            ));
        }
    }
    let (guid, owner) = match &slot.item {
        Some(item) => (item.guid.clone(), item.owner_lookup_id),
        None => (ArchiveText::new(&[])?, -1),
    };
    entries.extend([
        (h::ITEM_FLAG, RepValue::Bool(slot.item.is_some())),
        (h::ITEM_STRING, RepValue::Str(guid)),
        (h::ITEM_INTEGER, RepValue::Int(owner)),
        (h::ITEM_TAGS, RepValue::GameplayTagContainer(None)),
        (h::QUERY, RepValue::UInt16(slot.query_row)),
        (h::LOCKED, RepValue::Bool(false)),
        (h::LAST_ITEM, RepValue::UInt16(row)),
        (h::SLOTABLE, RepValue::Bool(false)),
        (h::INDEX, RepValue::Int(slot.index)),
    ]);
    Ok(item_stream(entries))
}

/// A component block: its bools and the caller's values and fields, or a
/// create-only block when it carries nothing.
fn component_block(
    component: &PawnComponent,
    layout: &[RepCommand],
    mut values: BTreeMap<u32, RepValue>,
    fields: Vec<CustomField>,
    field_bound: u32,
) -> Block {
    for (handle, value) in &component.rep_bools {
        values.insert(*handle, RepValue::Bool(*value));
    }
    if values.is_empty() && fields.is_empty() {
        return BlockContent::create_only().into();
    }
    Block::Composed(BlockProperties {
        layout: layout.to_vec(),
        references: BTreeMap::new(),
        values,
        fields,
        field_bound,
    })
}

/// A runtime `UInventory` subobject: active, its info row, its owning
/// component when `owner` names one, an explicitly supplied weight and its
/// slots as the custom-delta field (F-799, F-866, F-933). `owner` names the
/// `OwningComponent`, not the actor owner used by the weight tick gate.
pub fn inventory_subobject(
    blocks: &PawnContentPlan,
    inventory: &PawnInventory,
    owner: Option<Reference>,
    engine_net_version: u32,
    order: ByteOrder,
) -> Result<OpenSubobject> {
    let slots = fast_array_field(
        INVENTORY_SLOTS_FIELD_INDEX,
        inventory.slots.replication_key,
        inventory
            .slots
            .items
            .iter()
            .map(slot_stream)
            .collect::<Result<_>>()?,
        &blocks.slot_layout,
        engine_net_version,
        order,
    )?;
    let mut values = BTreeMap::from([
        (INVENTORY_IS_ACTIVE, RepValue::Bool(true)),
        (INVENTORY_INFO_ROW, RepValue::Byte(inventory.info_row)),
    ]);
    if let Some(weight) = inventory.current_weight {
        values.insert(INVENTORY_CURRENT_WEIGHT, RepValue::Int(weight));
    }
    Ok(OpenSubobject {
        class: blocks.inventory_class.clone(),
        block: Block::Composed(BlockProperties {
            layout: blocks.inventory_layout.clone(),
            references: owner
                .map(|owner| (INVENTORY_OWNING_COMPONENT, owner))
                .into_iter()
                .collect(),
            values,
            fields: vec![slots],
            field_bound: blocks.inventory_bound,
        }),
    })
}

/// Compose the pawn open for its owner's connection (F-797) on
/// `channel_index` in the connection's byte `order`: the captured roles and
/// references, then the state's content.
pub fn compose_pawn_open(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &PawnOpenPlan,
    state: &PawnOpenState,
    channel_index: u32,
    order: ByteOrder,
) -> Result<ComposedActorOpen> {
    for index in [
        plan.stat_container,
        plan.actor_state,
        plan.inventory_component,
    ] {
        ensure!(
            index < plan.components.len(),
            "pawn component index {index} out of range"
        );
    }
    let content = &state.content;
    ensure!(
        content
            .character_state
            .keys()
            .all(|handle| (3..=character_state_handle::LAST).contains(handle)),
        "character state handles lie in 3..=32"
    );
    let mut values = BTreeMap::from([
        (handle::REMOTE_ROLE, RepValue::Byte(OWNER_REMOTE_ROLE)),
        (handle::ROLE, RepValue::Byte(ROLE)),
    ]);
    if let Some(uid) = content.icarus_uid {
        values.insert(handle::ICARUS_UID, RepValue::Int(uid));
    }
    if !content.gameplay_tags.is_empty() {
        values.insert(
            handle::GAMEPLAY_TAGS,
            RepValue::GameplayTagContainer(Some(
                content
                    .gameplay_tags
                    .iter()
                    .map(|index| GameplayTag::NetIndex(*index))
                    .collect(),
            )),
        );
    }
    if content.frozen_movement {
        values.insert(handle::SERVER_FROZEN_MOVEMENT, RepValue::Bool(true));
    }
    let blocks = &plan.content;
    let stats = content
        .stats
        .as_ref()
        .map(|stats| {
            fast_array_field(
                STAT_ARRAY_FIELD_INDEX,
                stats.replication_key,
                stats
                    .items
                    .iter()
                    .map(|pair| item_stream(vec![(1, RepValue::StatPairRepState(*pair))]))
                    .collect(),
                &[RepCommand::StatPairRepState],
                plan.engine_net_version,
                order,
            )
        })
        .transpose()?;
    let components = plan
        .components
        .iter()
        .enumerate()
        .map(|(index, component)| {
            let block = if index == plan.stat_container {
                component_block(
                    component,
                    &blocks.component_layout,
                    BTreeMap::new(),
                    stats.iter().cloned().collect(),
                    blocks.stat_container_bound,
                )
            } else if index == plan.actor_state {
                component_block(
                    component,
                    &blocks.character_state_layout,
                    content.character_state.clone(),
                    Vec::new(),
                    1,
                )
            } else {
                component_block(
                    component,
                    &blocks.component_layout,
                    BTreeMap::new(),
                    Vec::new(),
                    1,
                )
            };
            OpenComponent {
                name: component.name.clone(),
                checksum: component.checksum,
                block: Some(block),
            }
        })
        .collect();
    let subobjects = content
        .inventories
        .iter()
        .map(|inventory| {
            inventory_subobject(
                blocks,
                inventory,
                Some(Reference::Component(plan.inventory_component)),
                plan.engine_net_version,
                order,
            )
        })
        .collect::<Result<_>>()?;
    let open = ComponentActorOpen {
        key: state.key,
        archetype: plan.archetype.clone(),
        level: plan.level.clone(),
        location: Some(state.location),
        layout: plan.layout.clone(),
        references: BTreeMap::from([
            (handle::OWNER, Reference::Guid(state.controller)),
            (handle::INSTIGATOR, Reference::Itself),
            (handle::PLAYER_STATE, Reference::Guid(state.player_state)),
            (handle::CONTROLLER, Reference::Guid(state.controller)),
            (
                handle::STAT_CONTAINER,
                Reference::Component(plan.stat_container),
            ),
            (handle::ACTOR_STATE, Reference::Component(plan.actor_state)),
            (
                handle::INVENTORY_COMPONENT,
                Reference::Component(plan.inventory_component),
            ),
        ]),
        values,
        components,
        subobjects,
        engine_net_version: plan.engine_net_version,
    };
    let (composed, _) = compose_component_actor_open(cache, exports, &open, channel_index, order)?;
    Ok(composed)
}

/// Frame a pawn property update carrying only `IcarusUID` (handle 46) on its
/// open channel: an actor bunch body without spawn holding one actor block.
/// The original changes a returning pawn's UID when its saved recorder
/// restores (F-950); `layout` is the pawn plan's. Nonpositive UIDs reject.
pub fn compose_pawn_icarus_uid_update(
    layout: &[RepCommand],
    engine_net_version: u32,
    icarus_uid: i32,
    order: ByteOrder,
) -> Result<Payload> {
    use super::actor_channel::{
        ActorBunchBody, BlockTarget, ContentBlock, encode_actor_bunch_body,
    };
    ensure!(icarus_uid > 0, "a pawn IcarusUID update is positive");
    let codec = actor_codec(engine_net_version, order);
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let stream = PropertyStream {
        entries: vec![RepEntry {
            handle: handle::ICARUS_UID,
            checksum: None,
            value: RepValue::Int(icarus_uid),
        }],
        terminator_checksum: None,
    };
    let block = ContentBlock {
        has_rep_layout: true,
        target: BlockTarget::Actor,
        payload: encode_rep_properties(&stream, layout, &rep_codec)?,
    };
    encode_actor_bunch_body(
        &ActorBunchBody {
            spawn: None,
            blocks: vec![block],
        },
        &codec,
    )
}
