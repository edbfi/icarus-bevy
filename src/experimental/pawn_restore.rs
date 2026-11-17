//! A returning player's pawn restored from its saved `C026` record (F-960).
//!
//! `UPlayerStateRecorderComponent::UpdateOwnerToDatabaseState` restores a
//! survival pawn in order: it teleports it to `Location`, sets the
//! controller's control rotation from `Rotation` (replicated on the pawn as
//! `ReplicatedControlRotation`), copies `RespawnCount` and
//! `HasGrantedLoadout`; a dead record only sets the unreplicated
//! `bWasDeadOnReload` and leaves a death transition to stitching, otherwise
//! a recorded `Health` goes through `SetHealth`; each recorded level goes
//! through its setter; the stomach is cleared and every valid saved entry
//! added; valid, unique cosmetic overrides replace the pawn's when any
//! remain. The setters clamp to the state's current replicated bounds
//! (F-961).
//!
//! [`restore_saved_pawn`] turns a record into the replicated values that
//! restore changes; [`compose_restored_pawn_update`] frames them as one
//! reliable pawn property update. Item-static row indices are the caller's
//! (the server data.pak's `D_ItemsStatic` order, F-830).
use std::collections::{BTreeMap, BTreeSet};

use anyhow::{Result, bail, ensure};

use super::actor_channel::{
    ActorBunchBody, BlockTarget, CompressedRotator, ContentBlock, NetGuidRef, SubObjectClass,
    encode_actor_bunch_body,
};
use super::archive::ByteOrder;
use super::inventory_restore::{InventoryRestore, SlotChange};
use super::inventory_weight::DEFAULT_CDO_CURRENT_WEIGHT;
use super::modifier_restore::RestoredModifier;
use super::packet::Payload;
use super::pawn_open::{
    INVENTORY_CURRENT_WEIGHT, INVENTORY_SLOTS_FIELD_INDEX, InventorySlot, PawnComponent,
    PawnOpenPlan, PawnTemperatureState, STAT_ARRAY_FIELD_INDEX, character_state_handle,
    fast_array_delta_field, handle, slot_stream,
};
use super::recorders::SavedPlayerState;
use super::rep_layout::{
    ICARUS_ROW_HANDLE_NONE, PropertyStream, RepArray, RepCommand, RepLayoutCodec, RepLayoutLimits,
    RepValue, encode_rep_properties, modifier_state_component_rep_layout,
    stomach_component_rep_layout, write_rep_properties,
};
use super::rep_rpc::{FastArrayDelta, FastArrayItem, FieldHeader, write_field_header};
use super::restore_rules::{UNRECORDED_LEVEL, filter_restored_cosmetic_armour_overrides};
use super::session_open::exports_of;
use super::session_open::{
    ActorKey, ComponentBlock, ComponentExport, ConnectionExports, NetGuidCache, actor_codec, entry,
};
use super::stat_restore::StatDelta;

/// `UStomachComponent::StomachContents` (`stomach_component_rep_layout`).
pub const STOMACH_CONTENTS_HANDLE: u32 = 3;

/// Why a saved record's restore is not composed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PawnRestoreGap {
    /// `bIsAlive` is false: the original sets only the unreplicated
    /// `bWasDeadOnReload` and kills the pawn at stitching, whose wire is
    /// unread (F-961).
    DeadOnSave,
}

/// A saved stomach entry whose food resolved to an item-static row.
#[derive(Clone, PartialEq, Eq)]
pub struct RestoredStomachContent {
    pub food_row: u16,
    /// The saved modifier row name.
    pub modifier_name: Vec<u16>,
}

impl std::fmt::Debug for RestoredStomachContent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("RestoredStomachContent")
    }
}

/// The replicated values a restore changes, by handle.
#[derive(Clone, PartialEq)]
pub struct RestoredPawn {
    /// The teleport target. Not composed: the teleport's wire is unread.
    pub location: [f32; 3],
    /// Pawn actor values (`ReplicatedControlRotation`, `RespawnCount`,
    /// `HasGrantedLoadout` and, when any remain, `CosmeticArmourOverrides`).
    /// Keys use canonical 3.0.31/32 handles; the composer maps them once to
    /// the selected release. Never place release-specific wire handles here.
    pub pawn: BTreeMap<u32, RepValue>,
    /// `UPlayerCharacterState` values that differ from the current ones.
    pub character_state: BTreeMap<u32, RepValue>,
    /// The restored inventories that change (F-978); empty when the save's
    /// inventories were not restored.
    pub inventories: Vec<InventoryRestore>,
    /// The restored modifiers, new runtime sub-objects (F-980).
    pub modifiers: Vec<RestoredModifier>,
    /// The restored stats' delta from the open's (F-988); `None` when they
    /// were not restored or did not change.
    pub stats: Option<StatDelta>,
    /// The stomach's new contents; `None` when it was and stays empty.
    pub stomach: Option<Vec<RestoredStomachContent>>,
}

impl std::fmt::Debug for RestoredPawn {
    // Restored values come from supplied saves; show handles and counts.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RestoredPawn")
            .field("pawn", &self.pawn.keys().collect::<Vec<_>>())
            .field(
                "character_state",
                &self.character_state.keys().collect::<Vec<_>>(),
            )
            .field("stomach", &self.stomach.as_ref().map(Vec::len))
            .field("inventories", &self.inventories.len())
            .field("modifiers", &self.modifiers.len())
            .field(
                "stats",
                &self.stats.as_ref().map(|stats| stats.changed.len()),
            )
            .finish_non_exhaustive()
    }
}

/// The pawn's current replicated state the restore's setters read.
#[derive(Clone, Copy, Debug)]
pub struct CurrentPawnState<'a> {
    /// `UPlayerCharacterState` values by handle, as the pawn last sent
    /// them (for a fresh pawn, [`super::pawn_open::PawnContent`]'s).
    pub character_state: &'a BTreeMap<u32, RepValue>,
    /// Entries in the pawn's stomach.
    pub stomach_contents: usize,
}

/// `SetOxygen`/`SetWater`/`SetFood`/`SetRadiation`: below `min` gives
/// `min`, at or above `max` gives `max` (F-961).
pub fn clamp_level(value: i32, min: i32, max: i32) -> i32 {
    if value < min {
        min
    } else if value < max {
        value
    } else {
        max
    }
}

/// `SetHealth`: negative gives 0, at or above `max_health` gives
/// `max_health` (F-961).
pub fn clamp_health(value: i32, max_health: i32) -> i32 {
    if value < 0 {
        0
    } else if value < max_health {
        value
    } else {
        max_health
    }
}

fn current_int(current: &BTreeMap<u32, RepValue>, handle: u32) -> Result<i32> {
    match current.get(&handle) {
        Some(RepValue::Int(value)) => Ok(*value),
        Some(_) => bail!("current character state handle {handle} is not an integer"),
        None => bail!("the current character state lacks handle {handle}"),
    }
}

/// A level bound as the pawn last sent it, else the constructed
/// `USurvivalCharacterState` default the open leaves unsent (F-963): every
/// minimum 0, `MaxOxygen`/`MaxWater`/`MaxFood` 100, `MaxRadiation` 1,000.
fn current_bound(current: &BTreeMap<u32, RepValue>, handle: u32) -> Result<i32> {
    use character_state_handle as h;
    if current.contains_key(&handle) {
        return current_int(current, handle);
    }
    Ok(match handle {
        h::MIN_OXYGEN | h::MIN_WATER | h::MIN_FOOD | h::MIN_RADIATION => 0,
        h::MAX_OXYGEN | h::MAX_WATER | h::MAX_FOOD => 100,
        h::MAX_RADIATION => 1_000,
        _ => bail!("no constructed default for character state handle {handle}"),
    })
}

/// An item-static row index that fits a 2-byte row handle naming a row.
fn item_row(
    resolve: &mut impl FnMut(&[u16]) -> Result<Option<usize>>,
    name: &[u16],
) -> Result<Option<u16>> {
    Ok(resolve(name)?
        .and_then(|row| u16::try_from(row).ok())
        .filter(|row| *row != ICARUS_ROW_HANDLE_NONE))
}

/// The replicated values `UpdateOwnerToDatabaseState` gives a survival pawn
/// from `saved` (F-960, F-961), or why it is not composed. `item_row_index`
/// resolves an item-static row name to its row index; a name it does not
/// resolve is an invalid row, dropped from the cosmetic overrides and the
/// stomach. Level and health bounds are `current`'s; a recorded level
/// whose bounds `current` lacks is an error.
pub fn restore_saved_pawn(
    saved: &SavedPlayerState,
    current: &CurrentPawnState<'_>,
    mut item_row_index: impl FnMut(&[u16]) -> Result<Option<usize>>,
) -> Result<std::result::Result<RestoredPawn, PawnRestoreGap>> {
    use character_state_handle as h;
    // A save omits fields equal to the recorder's constructed defaults, so an
    // omitted field restores as that default (F-963): zero placement,
    // RespawnCount 0, HasGrantedLoadout false, bIsAlive true, Health and
    // levels -1.
    let location = saved.location.unwrap_or([0.0; 3]);
    let [pitch, yaw, roll] = saved.rotation.unwrap_or([0.0; 3]);
    let respawn_count = saved.respawn_count.unwrap_or(0);
    let has_granted_loadout = saved.has_granted_loadout.unwrap_or(false);
    if saved.is_alive == Some(false) {
        return Ok(Err(PawnRestoreGap::DeadOnSave));
    }
    let health = saved.health.unwrap_or(UNRECORDED_LEVEL);
    // The setters' order: (saved, field, level, min, max handles).
    let mut levels = Vec::with_capacity(4);
    for (level, level_handle, min, max) in [
        (saved.oxygen_level, h::OXYGEN, h::MIN_OXYGEN, h::MAX_OXYGEN),
        (saved.food_level, h::FOOD, h::MIN_FOOD, h::MAX_FOOD),
        (saved.water_level, h::WATER, h::MIN_WATER, h::MAX_WATER),
        (
            saved.radiation_level,
            h::RADIATION,
            h::MIN_RADIATION,
            h::MAX_RADIATION,
        ),
    ] {
        let level = level.unwrap_or(UNRECORDED_LEVEL);
        if level != UNRECORDED_LEVEL {
            levels.push((level, level_handle, min, max));
        }
    }

    let mut pawn = BTreeMap::from([
        (
            handle::REPLICATED_CONTROL_ROTATION,
            RepValue::Rotator(CompressedRotator::from_degrees(pitch, yaw, roll)?),
        ),
        (handle::RESPAWN_COUNT, RepValue::Int(respawn_count)),
        (
            handle::HAS_GRANTED_LOADOUT,
            RepValue::Bool(has_granted_loadout),
        ),
    ]);

    let state = current.character_state;
    let mut character_state = BTreeMap::new();
    let mut set = |handle: u32, value: RepValue| {
        if state.get(&handle) != Some(&value) {
            character_state.insert(handle, value);
        }
    };
    if health != UNRECORDED_LEVEL {
        let health = clamp_health(health, current_int(state, h::MAX_HEALTH)?);
        set(h::HEALTH, RepValue::Int(health));
        set(
            h::CURRENT_ALIVE_STATE,
            RepValue::Enum(u32::from(health < 1)),
        );
    }
    for (level, level_handle, min, max) in levels {
        let (min, max) = (current_bound(state, min)?, current_bound(state, max)?);
        set(level_handle, RepValue::Int(clamp_level(level, min, max)));
    }

    let mut contents = Vec::new();
    for content in &saved.stomach_contents {
        if let Some(food_row) = item_row(&mut item_row_index, &content.food_row_name)? {
            contents.push(RestoredStomachContent {
                food_row,
                modifier_name: content.modifier_name.clone(),
            });
        }
    }
    let stomach = (current.stomach_contents > 0 || !contents.is_empty()).then_some(contents);

    let mut overrides = Vec::with_capacity(saved.cosmetic_armour_overrides.len());
    for name in &saved.cosmetic_armour_overrides {
        overrides.push(match item_row(&mut item_row_index, name)? {
            Some(row) => (row, true),
            None => (ICARUS_ROW_HANDLE_NONE, false),
        });
    }
    let (rows, apply) = filter_restored_cosmetic_armour_overrides(&overrides);
    if apply {
        pawn.insert(
            handle::COSMETIC_ARMOUR_OVERRIDES,
            RepValue::DynamicArray(RepArray {
                count: u16::try_from(rows.len())?,
                stream: PropertyStream {
                    entries: rows
                        .into_iter()
                        .zip(1..)
                        .map(|(row, element)| entry(element, RepValue::UInt16(row)))
                        .collect(),
                    terminator_checksum: None,
                },
            }),
        );
    }

    Ok(Ok(RestoredPawn {
        location,
        pawn,
        character_state,
        inventories: Vec::new(),
        modifiers: Vec::new(),
        stats: None,
        stomach,
    }))
}

/// One slot's properties by handle; an omitted array is its empty default.
fn slot_values(slot: &InventorySlot) -> Result<BTreeMap<u32, RepValue>> {
    use super::pawn_open::slot_handle as h;
    let empty = || {
        RepValue::DynamicArray(RepArray {
            count: 0,
            stream: PropertyStream {
                entries: Vec::new(),
                terminator_checksum: None,
            },
        })
    };
    let mut values: BTreeMap<u32, RepValue> = [h::ITEM_DYNAMIC, h::ITEM_STATS, h::ITEM_ALTERATIONS]
        .into_iter()
        .map(|handle| (handle, empty()))
        .collect();
    for entry in slot_stream(slot)?.entries {
        values.insert(entry.handle, entry.value);
    }
    Ok(values)
}

/// A restored inventory's block (F-978): its runtime subobject with its
/// class, and the slots fast array's delta from the key the client holds;
/// a changed slot sends the properties that differ from its open, an added
/// slot every property. A restored `CurrentWeight` that differs from the
/// open's goes as a property ahead of the slots (F-982).
fn restored_inventory_block(
    cache: &mut NetGuidCache,
    plan: &PawnOpenPlan,
    key: ActorKey,
    class: u32,
    inventory: &InventoryRestore,
    codec: &RepLayoutCodec,
) -> Result<ContentBlock> {
    let order = codec.actor.order;
    let mut changed = Vec::with_capacity(inventory.changes.len());
    for (id, change) in &inventory.changes {
        let stream = match change {
            SlotChange::Added(slot) => slot_stream(slot)?,
            SlotChange::Changed(slot) => {
                let index = usize::try_from(*id - 1)?;
                let before = slot_values(
                    inventory
                        .opened
                        .slots
                        .items
                        .get(index)
                        .ok_or_else(|| anyhow::anyhow!("a changed slot the open lacks"))?,
                )?;
                let after = slot_values(slot)?;
                PropertyStream {
                    entries: after
                        .into_iter()
                        .filter(|(handle, value)| before.get(handle) != Some(value))
                        .map(|(handle, value)| entry(handle, value))
                        .collect(),
                    terminator_checksum: None,
                }
            }
        };
        changed.push((*id, FastArrayItem::Delta(stream)));
    }
    let field = fast_array_delta_field(
        INVENTORY_SLOTS_FIELD_INDEX,
        &FastArrayDelta {
            array_replication_key: inventory.restored.slots.replication_key,
            base_replication_key: inventory.base_key,
            deleted_ids: Vec::new(),
            changed,
        },
        &plan.content.slot_layout,
        plan.engine_net_version,
        order,
    )?;
    let mut writer = super::packet::BitWriter::new();
    // A changed weight goes as a property ahead of the slots (F-982); an
    // open without one holds the CDO's.
    let weight = inventory.weight.filter(|weight| {
        *weight
            != inventory
                .opened
                .current_weight
                .unwrap_or(DEFAULT_CDO_CURRENT_WEIGHT)
    });
    if let Some(weight) = weight {
        write_rep_properties(
            &mut writer,
            &PropertyStream {
                entries: vec![entry(INVENTORY_CURRENT_WEIGHT, RepValue::Int(weight))],
                terminator_checksum: None,
            },
            &plan.content.inventory_layout,
            codec,
        )?;
    }
    write_field_header(
        &mut writer,
        &FieldHeader {
            index: field.index,
            payload: field.payload,
        },
        plan.content.inventory_bound,
    )?;
    Ok(ContentBlock {
        has_rep_layout: weight.is_some(),
        target: BlockTarget::SubObject {
            object: NetGuidRef::plain(
                cache.subobject_guid(key, u32::try_from(inventory.inventory)?)?,
            ),
            class: SubObjectClass::Class(NetGuidRef::plain(class)),
        },
        payload: Payload::from_writer(writer),
    })
}

/// The stat container's block (F-988): its replicated stat array's delta
/// from the key the client holds, the stably-named component's only field.
fn restored_stat_block(
    plan: &PawnOpenPlan,
    guid: u32,
    stats: &StatDelta,
    order: ByteOrder,
) -> Result<ContentBlock> {
    let field = fast_array_delta_field(
        STAT_ARRAY_FIELD_INDEX,
        &FastArrayDelta {
            array_replication_key: stats.array_key,
            base_replication_key: stats.base_key,
            deleted_ids: Vec::new(),
            changed: stats
                .changed
                .iter()
                .map(|(id, pair)| {
                    (
                        *id,
                        FastArrayItem::Delta(PropertyStream {
                            entries: vec![entry(1, RepValue::StatPairRepState(*pair))],
                            terminator_checksum: None,
                        }),
                    )
                })
                .collect(),
        },
        &[RepCommand::StatPairRepState],
        plan.engine_net_version,
        order,
    )?;
    let mut writer = super::packet::BitWriter::new();
    write_field_header(
        &mut writer,
        &FieldHeader {
            index: field.index,
            payload: field.payload,
        },
        plan.content.stat_container_bound,
    )?;
    Ok(ContentBlock {
        has_rep_layout: false,
        target: BlockTarget::SubObject {
            object: NetGuidRef::plain(guid),
            class: SubObjectClass::StablyNamed,
        },
        payload: Payload::from_writer(writer),
    })
}

/// A pawn component's GUID with its path while the connection still needs
/// an export (F-796), including after its open's export packet was lost.
fn component_reference(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    key: ActorKey,
    actor: u32,
    component: &PawnComponent,
) -> Result<NetGuidRef> {
    cache.export_keyed_subobject(
        exports,
        key,
        actor,
        &ComponentExport {
            name: component.name.clone(),
            checksum: component.checksum,
            block: ComponentBlock::None,
        },
    )
}

fn rep_block(
    target: BlockTarget,
    values: &BTreeMap<u32, RepValue>,
    layout: &[RepCommand],
    codec: &RepLayoutCodec,
) -> Result<ContentBlock> {
    let stream = PropertyStream {
        entries: values
            .iter()
            .map(|(handle, value)| entry(*handle, value.clone()))
            .collect(),
        terminator_checksum: None,
    };
    Ok(ContentBlock {
        has_rep_layout: true,
        target,
        payload: encode_rep_properties(&stream, layout, codec)?,
    })
}

/// Frame `restored` as one pawn property update on the pawn's open channel
/// for the connection `exports` tracks: an actor bunch body without spawn
/// holding the pawn's actor block, then its `ActorState` and, when the
/// stomach changes, its stomach component (`stomach_component`, an index
/// into the plan's components) blocks, each only when it carries values,
/// then each restored inventory's slot delta and each restored modifier as a
/// new class-named sub-object, with the classes the connection has not
/// acked exported ahead of the body; restored stats go as the stat
/// container's delta right after the actor block (F-989). The caller sends
/// the update reliably after the pawn's open; any unacknowledged component
/// paths are exported ahead of its body. Restored stomach entries reject:
/// their `ModifierID` comes from `AddSaveDataToStomach`, which is unread; clearing
/// a stomach composes.
pub fn compose_restored_pawn_update(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &PawnOpenPlan,
    key: ActorKey,
    stomach_component: usize,
    restored: &RestoredPawn,
    order: ByteOrder,
) -> Result<RestoredPawnUpdate> {
    ensure!(
        plan.actor_state < plan.components.len() && stomach_component < plan.components.len(),
        "pawn component index out of range"
    );
    ensure!(
        restored
            .character_state
            .keys()
            .all(|handle| (3..=character_state_handle::LAST).contains(handle)),
        "character state handles lie in 3..=32"
    );
    ensure!(
        restored.stomach.as_ref().is_none_or(Vec::is_empty),
        "restored stomach entries are not composed: AddSaveDataToStomach is unread"
    );
    let codec = actor_codec(plan.engine_net_version, order);
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let pawn_values = restored
        .pawn
        .iter()
        .map(|(&handle, value)| Ok((plan.layout_version.wire_handle(handle)?, value.clone())))
        .collect::<Result<BTreeMap<_, _>>>()?;
    exports.begin_bunch();
    let actor = cache.actor_guid(key)?;
    let mut blocks = Vec::new();
    let mut exported = Vec::new();
    if !pawn_values.is_empty() {
        blocks.push(rep_block(
            BlockTarget::Actor,
            &pawn_values,
            &plan.layout,
            &rep_codec,
        )?);
    }
    // The stat container's block follows the actor's, as the original's
    // restore bunch orders them.
    if let Some(stats) = &restored.stats {
        let reference = component_reference(
            cache,
            exports,
            key,
            actor,
            &plan.components[plan.stat_container],
        )?;
        blocks.push(restored_stat_block(plan, reference.guid, stats, order)?);
        exported.push(reference);
    }
    let mut component_block = |index: usize, values: &BTreeMap<u32, RepValue>, layout| {
        let reference = component_reference(cache, exports, key, actor, &plan.components[index])?;
        let guid = reference.guid;
        exported.push(reference);
        rep_block(
            BlockTarget::SubObject {
                object: NetGuidRef::plain(guid),
                class: SubObjectClass::StablyNamed,
            },
            values,
            layout,
            &rep_codec,
        )
    };
    if !restored.character_state.is_empty() {
        blocks.push(component_block(
            plan.actor_state,
            &restored.character_state,
            &plan.content.character_state_layout,
        )?);
    }
    if restored.stomach.is_some() {
        let cleared = BTreeMap::from([(
            STOMACH_CONTENTS_HANDLE,
            RepValue::DynamicArray(RepArray {
                count: 0,
                stream: PropertyStream {
                    entries: Vec::new(),
                    terminator_checksum: None,
                },
            }),
        )]);
        blocks.push(component_block(
            stomach_component,
            &cleared,
            &stomach_component_rep_layout(),
        )?);
    }
    // Classes share the prefix with the required component paths.
    if !restored.inventories.is_empty() {
        let class = cache.export_static(exports, &plan.content.inventory_class)?;
        let guid = class.guid;
        exported.push(class);
        for inventory in &restored.inventories {
            blocks.push(restored_inventory_block(
                cache, plan, key, guid, inventory, &rep_codec,
            )?);
        }
    }
    for modifier in &restored.modifiers {
        let class = cache.export_static(exports, &modifier.class)?;
        let class_guid = class.guid;
        exported.push(class);
        let mut values = BTreeMap::from([
            (2, RepValue::Bool(true)),
            (3, RepValue::UInt16(modifier.row)),
            (4, RepValue::Object(NetGuidRef::plain(actor))),
            (5, RepValue::Int(modifier.uid)),
            (6, RepValue::Float(modifier.lifetime.to_bits())),
        ]);
        if modifier.final_effectiveness != 0 {
            values.insert(7, RepValue::Int(modifier.final_effectiveness));
        }
        if modifier.remaining_tenths != 0 {
            values.insert(8, RepValue::UInt16(modifier.remaining_tenths));
        }
        blocks.push(rep_block(
            BlockTarget::SubObject {
                object: NetGuidRef::plain(cache.subobject_guid(key, modifier.subobject)?),
                class: SubObjectClass::Class(NetGuidRef::plain(class_guid)),
            },
            &values,
            &modifier_state_component_rep_layout(),
            &rep_codec,
        )?);
    }
    ensure!(!blocks.is_empty(), "a restored pawn update carries values");
    Ok(RestoredPawnUpdate {
        exports: exports_of(&exported, &codec)?,
        payload: encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks,
            },
            &codec,
        )?,
    })
}

/// Compose explicit remaining-time values for previously restored modifiers.
/// The caller retains their UID, subobject index and class identities and waits
/// until the initial reliable restore has been acknowledged before sending this
/// update. This encodes the supplied wire field, including zero, independently
/// of raw seconds; it neither runs a setter nor assumes callback effects.
/// On failure, the GUID cache and connection exports are unchanged.
pub fn compose_modifier_time_update(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &PawnOpenPlan,
    key: ActorKey,
    modifiers: &[RestoredModifier],
    order: ByteOrder,
) -> Result<RestoredPawnUpdate> {
    compose_live_pawn_update(cache, exports, plan, key, None, modifiers, order)
}

/// Compose explicitly supplied temperatures and modifier wire times as one
/// pawn update. Temperatures use the pawn's existing ActorState component;
/// modifiers retain their original runtime object/class identities. Unknown
/// temperature fields are omitted, while explicit zeroes are encoded.
/// Callers own calculations, callback effects and reliable delivery ordering.
/// On failure neither the GUID cache nor connection exports are changed.
pub fn compose_live_pawn_update(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &PawnOpenPlan,
    key: ActorKey,
    temperatures: Option<&PawnTemperatureState>,
    modifiers: &[RestoredModifier],
    order: ByteOrder,
) -> Result<RestoredPawnUpdate> {
    compose_live_pawn_vitals_update(
        cache,
        exports,
        plan,
        key,
        temperatures,
        modifiers,
        None,
        order,
    )
}

/// Extend the existing reliable live component update with a complete
/// server-owned vital checkpoint. This changes CharacterState values only;
/// checkpoint placement is applied by the next pawn open.
#[expect(clippy::too_many_arguments)]
pub fn compose_live_pawn_vitals_update(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &PawnOpenPlan,
    key: ActorKey,
    temperatures: Option<&PawnTemperatureState>,
    modifiers: &[RestoredModifier],
    vitals: Option<&super::pawn_open::CharacterVitals>,
    order: ByteOrder,
) -> Result<RestoredPawnUpdate> {
    compose_live_pawn_bag_update(
        cache,
        exports,
        plan,
        key,
        temperatures,
        modifiers,
        vitals,
        &[],
        order,
    )
}

/// [`compose_live_pawn_vitals_update`] plus explicit slot deltas of inventories
/// the pawn opened, as a restore sends them (F-978): each delta is relative to
/// the slots and key its `opened` says the client holds, carries no weight,
/// and goes on that inventory's runtime subobject after the character state
/// and before the modifiers, as the restore orders them (F-1056). Without
/// inventories the bytes equal [`compose_live_pawn_vitals_update`]'s. The
/// caller owns which bag the deltas describe and their reliable delivery. On
/// failure neither the GUID cache nor connection exports are changed.
#[expect(clippy::too_many_arguments)]
pub fn compose_live_pawn_bag_update(
    cache: &mut NetGuidCache,
    exports: &mut ConnectionExports,
    plan: &PawnOpenPlan,
    key: ActorKey,
    temperatures: Option<&PawnTemperatureState>,
    modifiers: &[RestoredModifier],
    vitals: Option<&super::pawn_open::CharacterVitals>,
    inventories: &[InventoryRestore],
    order: ByteOrder,
) -> Result<RestoredPawnUpdate> {
    let mut seen_inventories = BTreeSet::new();
    for inventory in inventories {
        ensure!(
            seen_inventories.insert(inventory.inventory),
            "duplicate live inventory delta"
        );
        ensure!(
            inventory.weight.is_none(),
            "a live inventory delta carries no weight"
        );
        ensure!(
            !inventory.changes.is_empty()
                && inventory
                    .changes
                    .iter()
                    .all(|(_, change)| matches!(change, SlotChange::Changed(_))),
            "a live inventory delta changes known slots"
        );
    }
    let mut temperature_values = temperatures
        .map(PawnTemperatureState::values)
        .unwrap_or_default();
    if let Some(vitals) = vitals {
        temperature_values.extend(vitals.values());
        temperature_values.insert(
            character_state_handle::CURRENT_ALIVE_STATE,
            RepValue::Enum(u32::from(vitals.health < 1)),
        );
    }
    ensure!(
        !temperature_values.is_empty() || !modifiers.is_empty() || !inventories.is_empty(),
        "a live pawn update carries values"
    );
    if !temperature_values.is_empty() {
        ensure!(
            plan.actor_state < plan.components.len(),
            "pawn component index out of range"
        );
    }
    let mut uids = BTreeSet::new();
    let mut subobjects = BTreeSet::new();
    for modifier in modifiers {
        ensure!(uids.insert(modifier.uid), "duplicate modifier UID");
        ensure!(
            subobjects.insert(modifier.subobject),
            "duplicate modifier subobject"
        );
        ensure!(!modifier.class.is_empty(), "empty modifier class");
    }
    let codec = actor_codec(plan.engine_net_version, order);
    let rep_codec = RepLayoutCodec {
        actor: codec,
        checksums: false,
        limits: RepLayoutLimits::default(),
    };
    let mut next_cache = cache.clone();
    let mut next_exports = exports.clone();
    next_exports.begin_bunch();
    let mut blocks = Vec::new();
    let mut exported = Vec::new();
    if !temperature_values.is_empty() {
        let actor = next_cache.actor_guid(key)?;
        let reference = component_reference(
            &mut next_cache,
            &mut next_exports,
            key,
            actor,
            &plan.components[plan.actor_state],
        )?;
        blocks.push(rep_block(
            BlockTarget::SubObject {
                object: NetGuidRef::plain(reference.guid),
                class: SubObjectClass::StablyNamed,
            },
            &temperature_values,
            &plan.content.character_state_layout,
            &rep_codec,
        )?);
        exported.push(reference);
    }
    if !inventories.is_empty() {
        let class = next_cache.export_static(&mut next_exports, &plan.content.inventory_class)?;
        let class_guid = class.guid;
        exported.push(class);
        for inventory in inventories {
            blocks.push(restored_inventory_block(
                &mut next_cache,
                plan,
                key,
                class_guid,
                inventory,
                &rep_codec,
            )?);
        }
    }
    for modifier in modifiers {
        let class = next_cache.export_static(&mut next_exports, &modifier.class)?;
        let class_guid = class.guid;
        exported.push(class);
        blocks.push(rep_block(
            BlockTarget::SubObject {
                object: NetGuidRef::plain(next_cache.subobject_guid(key, modifier.subobject)?),
                class: SubObjectClass::Class(NetGuidRef::plain(class_guid)),
            },
            &BTreeMap::from([(8, RepValue::UInt16(modifier.remaining_tenths))]),
            &modifier_state_component_rep_layout(),
            &rep_codec,
        )?);
    }
    let update = RestoredPawnUpdate {
        exports: exports_of(&exported, &codec)?,
        payload: encode_actor_bunch_body(
            &ActorBunchBody {
                spawn: None,
                blocks,
            },
            &codec,
        )?,
    };
    *cache = next_cache;
    *exports = next_exports;
    Ok(update)
}

/// A restored pawn update: its body and the export-only prefix the body's
/// component and class references need, if any.
#[derive(Clone, Debug)]
pub struct RestoredPawnUpdate {
    pub exports: Option<Payload>,
    pub payload: Payload,
}

#[cfg(test)]
mod live_bag_update_tests {
    //! Authored rows and slots only; decoded with the repository's readers.
    use super::super::actor_channel::read_actor_bunch_body;
    use super::super::pawn_open::{
        CharacterVitals, FastArrayContent, PawnContent, PawnInventory, PawnOpenState, SlotItem,
        compose_pawn_open,
    };
    use super::super::peer_lab::PeerLabAdmission;
    use super::super::release::ReleaseProfile;
    use super::super::rep_layout::read_rep_properties;
    use super::super::rep_rpc::{
        FAST_ARRAY_COUNT_CAP, FastArrayItem, FastArrayItemLayout, read_custom_delta_header,
        read_fast_array_delta, read_field_header,
    };
    use super::*;

    const KEY: ActorKey = ActorKey::Session(7);

    fn plan() -> PawnOpenPlan {
        ReleaseProfile::fixture("3.0.32")
            .pawn_open_plan(PeerLabAdmission::default().map_url)
            .unwrap()
            .unwrap()
    }

    fn empty(info_row: u8, slots: i32, query: u16) -> PawnInventory {
        PawnInventory {
            info_row,
            current_weight: None,
            slots: FastArrayContent {
                replication_key: 2,
                items: (0..slots)
                    .map(|index| InventorySlot::empty(query, index))
                    .collect(),
            },
        }
    }

    fn vitals() -> CharacterVitals {
        CharacterVitals {
            health: 90,
            max_health: 100,
            stamina: 100,
            max_stamina: 100,
            oxygen: 40,
            max_oxygen: 100,
            water: 50,
            max_water: 100,
            food: 60,
            max_food: 100,
            biome_row: 0,
            growth_row: 0,
        }
    }

    /// A pawn opened with one listed inventory and one projected (index 1),
    /// its exports acked.
    fn opened(order: ByteOrder) -> (PawnOpenPlan, NetGuidCache, ConnectionExports) {
        let plan = plan();
        let mut cache = NetGuidCache::new();
        let mut exports = ConnectionExports::new();
        compose_pawn_open(
            &mut cache,
            &mut exports,
            &plan,
            &PawnOpenState {
                key: KEY,
                controller: 2,
                player_state: 4,
                location: [0.0; 3],
                content: PawnContent {
                    inventories: vec![empty(0, 1, 9), empty(5, 3, 9)],
                    ..PawnContent::default()
                },
            },
            3,
            order,
        )
        .unwrap();
        exports.commit_bunch(1);
        exports.received_ack(1);
        (plan, cache, exports)
    }

    fn item(row: u16, count: i32) -> Option<SlotItem> {
        Some(SlotItem {
            dynamic: vec![(0, 0), (7, count)],
            ..SlotItem::new(row).unwrap()
        })
    }

    /// The held inventory 1 with `items`, at `key`.
    fn held(items: [Option<SlotItem>; 3], key: i32) -> PawnInventory {
        let mut inventory = empty(5, 3, 9);
        for (slot, item) in inventory.slots.items.iter_mut().zip(items) {
            slot.item = item;
        }
        inventory.slots.replication_key = key;
        inventory
    }

    /// A delta from `from` to `to`, one key per changed slot (F-978).
    fn delta(from: &PawnInventory, to: &PawnInventory) -> InventoryRestore {
        let changes: Vec<_> = from
            .slots
            .items
            .iter()
            .zip(&to.slots.items)
            .zip(1..)
            .filter(|((before, after), _)| before != after)
            .map(|((_, after), id)| (id, SlotChange::Changed(after.clone())))
            .collect();
        let mut restored = to.clone();
        restored.slots.replication_key = from.slots.replication_key + changes.len() as i32;
        InventoryRestore {
            inventory: 1,
            opened: from.clone(),
            restored,
            base_key: from.slots.replication_key,
            changes,
            weight: None,
        }
    }

    /// Decode a live update: its CharacterState values (if any) and the
    /// inventory-1 slot delta, checking the block's target and class.
    fn decode(
        plan: &PawnOpenPlan,
        cache: &mut NetGuidCache,
        exports: &ConnectionExports,
        update: &RestoredPawnUpdate,
        order: ByteOrder,
    ) -> (BTreeMap<u32, RepValue>, FastArrayDelta) {
        let codec = RepLayoutCodec {
            actor: actor_codec(plan.engine_net_version, order),
            checksums: false,
            limits: RepLayoutLimits::default(),
        };
        let body =
            read_actor_bunch_body(&mut update.payload.reader(), false, &codec.actor).unwrap();
        let (vitals, inventory) = match body.blocks.as_slice() {
            [state, inventory] => (
                read_rep_properties(
                    &mut state.payload.reader(),
                    &plan.content.character_state_layout,
                    &codec,
                )
                .unwrap()
                .entries
                .into_iter()
                .map(|entry| (entry.handle, entry.value))
                .collect(),
                inventory,
            ),
            [inventory] => (BTreeMap::new(), inventory),
            blocks => panic!("{} blocks", blocks.len()),
        };
        let BlockTarget::SubObject { object, class } = &inventory.target else {
            panic!("inventory subobject block");
        };
        assert_eq!(object.guid, cache.subobject_guid(KEY, 1).unwrap());
        let SubObjectClass::Class(class) = class else {
            panic!("class-named inventory");
        };
        assert!(exports.is_acked(class.guid), "the open exported the class");
        assert!(!inventory.has_rep_layout, "no weight property");
        let mut reader = inventory.payload.reader();
        let field = read_field_header(&mut reader, plan.content.inventory_bound, 1 << 20).unwrap();
        assert!(reader.at_end());
        assert_eq!(field.index, INVENTORY_SLOTS_FIELD_INDEX);
        let mut field_reader = field.payload.reader();
        read_custom_delta_header(&mut field_reader, plan.engine_net_version, 1).unwrap();
        let delta = read_fast_array_delta(
            &mut field_reader,
            &FastArrayItemLayout::Delta(plan.content.slot_layout.clone()),
            &codec,
            FAST_ARRAY_COUNT_CAP,
        )
        .unwrap();
        assert!(field_reader.at_end());
        (vitals, delta)
    }

    /// Apply a decoded delta to the client's slot properties.
    fn apply(client: &mut [BTreeMap<u32, RepValue>], delta: &FastArrayDelta) {
        assert!(delta.deleted_ids.is_empty());
        for (id, item) in &delta.changed {
            let FastArrayItem::Delta(stream) = item else {
                panic!("delta items");
            };
            let slot = &mut client[usize::try_from(*id - 1).unwrap()];
            for entry in &stream.entries {
                slot.insert(entry.handle, entry.value.clone());
            }
        }
    }

    fn client_of(inventory: &PawnInventory) -> Vec<BTreeMap<u32, RepValue>> {
        inventory
            .slots
            .items
            .iter()
            .map(|slot| slot_values(slot).unwrap())
            .collect()
    }

    /// FROZEN pre-projection reference: `compose_live_pawn_vitals_update`
    /// exactly as it was at e9a36150, before the bag projection, kept
    /// verbatim (renamed only) so the bag composer is checked against an
    /// independent copy rather than against itself. Do not edit.
    #[expect(clippy::too_many_arguments)]
    fn head_vitals_update(
        cache: &mut NetGuidCache,
        exports: &mut ConnectionExports,
        plan: &PawnOpenPlan,
        key: ActorKey,
        temperatures: Option<&PawnTemperatureState>,
        modifiers: &[RestoredModifier],
        vitals: Option<&super::super::pawn_open::CharacterVitals>,
        order: ByteOrder,
    ) -> Result<RestoredPawnUpdate> {
        let mut temperature_values = temperatures
            .map(PawnTemperatureState::values)
            .unwrap_or_default();
        if let Some(vitals) = vitals {
            temperature_values.extend(vitals.values());
            temperature_values.insert(
                character_state_handle::CURRENT_ALIVE_STATE,
                RepValue::Enum(u32::from(vitals.health < 1)),
            );
        }
        ensure!(
            !temperature_values.is_empty() || !modifiers.is_empty(),
            "a live pawn update carries values"
        );
        if !temperature_values.is_empty() {
            ensure!(
                plan.actor_state < plan.components.len(),
                "pawn component index out of range"
            );
        }
        let mut uids = BTreeSet::new();
        let mut subobjects = BTreeSet::new();
        for modifier in modifiers {
            ensure!(uids.insert(modifier.uid), "duplicate modifier UID");
            ensure!(
                subobjects.insert(modifier.subobject),
                "duplicate modifier subobject"
            );
            ensure!(!modifier.class.is_empty(), "empty modifier class");
        }
        let codec = actor_codec(plan.engine_net_version, order);
        let rep_codec = RepLayoutCodec {
            actor: codec,
            checksums: false,
            limits: RepLayoutLimits::default(),
        };
        let mut next_cache = cache.clone();
        let mut next_exports = exports.clone();
        next_exports.begin_bunch();
        let mut blocks = Vec::new();
        let mut exported = Vec::new();
        if !temperature_values.is_empty() {
            let actor = next_cache.actor_guid(key)?;
            let reference = component_reference(
                &mut next_cache,
                &mut next_exports,
                key,
                actor,
                &plan.components[plan.actor_state],
            )?;
            blocks.push(rep_block(
                BlockTarget::SubObject {
                    object: NetGuidRef::plain(reference.guid),
                    class: SubObjectClass::StablyNamed,
                },
                &temperature_values,
                &plan.content.character_state_layout,
                &rep_codec,
            )?);
            exported.push(reference);
        }
        for modifier in modifiers {
            let class = next_cache.export_static(&mut next_exports, &modifier.class)?;
            let class_guid = class.guid;
            exported.push(class);
            blocks.push(rep_block(
                BlockTarget::SubObject {
                    object: NetGuidRef::plain(next_cache.subobject_guid(key, modifier.subobject)?),
                    class: SubObjectClass::Class(NetGuidRef::plain(class_guid)),
                },
                &BTreeMap::from([(8, RepValue::UInt16(modifier.remaining_tenths))]),
                &modifier_state_component_rep_layout(),
                &rep_codec,
            )?);
        }
        let update = RestoredPawnUpdate {
            exports: exports_of(&exported, &codec)?,
            payload: encode_actor_bunch_body(
                &ActorBunchBody {
                    spawn: None,
                    blocks,
                },
                &codec,
            )?,
        };
        *cache = next_cache;
        *exports = next_exports;
        Ok(update)
    }

    #[test]
    fn without_inventories_the_bytes_are_the_vitals_update() {
        let mut dead = vitals();
        dead.health = 0;
        let temperature = PawnTemperatureState {
            external: Some(21),
            modified_external: None,
            modified_internal: Some(-3),
            internal: Some(19),
        };
        let cases: [(&str, CharacterVitals, Option<PawnTemperatureState>); 3] = [
            ("living", vitals(), None),
            ("dead", dead, None),
            ("living with temperatures", vitals(), Some(temperature)),
        ];
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let (plan, cache, exports) = opened(order);
            for (name, vitals, temperatures) in &cases {
                let (mut ref_cache, mut ref_exports) = (cache.clone(), exports.clone());
                let reference = head_vitals_update(
                    &mut ref_cache,
                    &mut ref_exports,
                    &plan,
                    KEY,
                    temperatures.as_ref(),
                    &[],
                    Some(vitals),
                    order,
                )
                .unwrap();
                for current in [true, false] {
                    let (mut cache, mut exports) = (cache.clone(), exports.clone());
                    let update = if current {
                        compose_live_pawn_bag_update(
                            &mut cache,
                            &mut exports,
                            &plan,
                            KEY,
                            temperatures.as_ref(),
                            &[],
                            Some(vitals),
                            &[],
                            order,
                        )
                    } else {
                        compose_live_pawn_vitals_update(
                            &mut cache,
                            &mut exports,
                            &plan,
                            KEY,
                            temperatures.as_ref(),
                            &[],
                            Some(vitals),
                            order,
                        )
                    }
                    .unwrap();
                    let at = format!("{name}, {order:?}, bag composer {current}");
                    assert_eq!(update.payload, reference.payload, "bytes: {at}");
                    assert_eq!(update.exports, reference.exports, "exports: {at}");
                    assert_eq!(
                        format!("{cache:?}"),
                        format!("{ref_cache:?}"),
                        "GUID cache effect: {at}"
                    );
                    assert_eq!(
                        format!("{exports:?}"),
                        format!("{ref_exports:?}"),
                        "connection export effect: {at}"
                    );
                }
            }
            // A dead player's update marks it dead; a living one alive.
            let alive_state = |vitals: &CharacterVitals| {
                let (mut cache, mut exports) = (cache.clone(), exports.clone());
                let update = head_vitals_update(
                    &mut cache,
                    &mut exports,
                    &plan,
                    KEY,
                    None,
                    &[],
                    Some(vitals),
                    order,
                )
                .unwrap();
                let codec = RepLayoutCodec {
                    actor: actor_codec(plan.engine_net_version, order),
                    checksums: false,
                    limits: RepLayoutLimits::default(),
                };
                let body = read_actor_bunch_body(&mut update.payload.reader(), false, &codec.actor)
                    .unwrap();
                let [state] = body.blocks.as_slice() else {
                    panic!("one CharacterState block");
                };
                read_rep_properties(
                    &mut state.payload.reader(),
                    &plan.content.character_state_layout,
                    &codec,
                )
                .unwrap()
                .entries
                .into_iter()
                .find(|entry| entry.handle == character_state_handle::CURRENT_ALIVE_STATE)
                .unwrap()
                .value
            };
            assert_eq!(alive_state(&vitals()), RepValue::Enum(0));
            assert_eq!(alive_state(&dead), RepValue::Enum(1));
        }
    }

    #[test]
    fn slot_deltas_round_trip_after_the_vitals_in_both_byte_orders() {
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let (plan, mut cache, mut exports) = opened(order);
            // F-978: the client holds 2 + 3 after the open.
            let open = held([None, None, None], 5);
            let mut client = client_of(&open);
            let starter = held([item(40, 3), None, item(41, 1)], 0);
            let first = delta(&open, &starter);
            assert_eq!(
                (first.base_key, first.restored.slots.replication_key),
                (5, 7)
            );
            let update = compose_live_pawn_bag_update(
                &mut cache,
                &mut exports,
                &plan,
                KEY,
                None,
                &[],
                Some(&vitals()),
                std::slice::from_ref(&first),
                order,
            )
            .unwrap();
            assert!(update.exports.is_none(), "every reference was acked");
            let (values, decoded) = decode(&plan, &mut cache, &exports, &update, order);
            let mut alive = vitals().values();
            alive.insert(
                character_state_handle::CURRENT_ALIVE_STATE,
                RepValue::Enum(0),
            );
            assert_eq!(values, alive, "vitals first");
            assert_eq!(
                (decoded.array_replication_key, decoded.base_replication_key),
                (7, 5)
            );
            assert_eq!(
                decoded
                    .changed
                    .iter()
                    .map(|(id, _)| *id)
                    .collect::<Vec<_>>(),
                [1, 3]
            );
            // A filled slot sends the item, its data, the flag and LastItem.
            let FastArrayItem::Delta(stream) = &decoded.changed[0].1 else {
                panic!("delta item");
            };
            assert_eq!(
                stream.entries.iter().map(|e| e.handle).collect::<Vec<_>>(),
                [2, 3, 10, 16]
            );
            apply(&mut client, &decoded);
            assert_eq!(client, client_of(&starter), "{order:?}");
            // Use empties slot 2 and the first stack drops to 2: +2.
            let mut used = held([item(40, 2), None, None], 7);
            let second = delta(&first.restored, &used);
            used.slots.replication_key = 9;
            let update = compose_live_pawn_bag_update(
                &mut cache,
                &mut exports,
                &plan,
                KEY,
                None,
                &[],
                None,
                std::slice::from_ref(&second),
                order,
            )
            .unwrap();
            let (values, decoded) = decode(&plan, &mut cache, &exports, &update, order);
            assert!(values.is_empty(), "no character state block");
            assert_eq!(
                (decoded.array_replication_key, decoded.base_replication_key),
                (9, 7)
            );
            let FastArrayItem::Delta(count) = &decoded.changed[0].1 else {
                panic!("delta item");
            };
            assert_eq!(
                count.entries.iter().map(|e| e.handle).collect::<Vec<_>>(),
                [3],
                "only the dynamic data changes with the count"
            );
            apply(&mut client, &decoded);
            assert_eq!(client, client_of(&used), "{order:?}");
        }
    }

    #[test]
    fn weights_added_slots_empty_or_repeated_deltas_refuse_without_state_change() {
        let order = ByteOrder::Little;
        let (plan, cache, exports) = opened(order);
        let open = held([None, None, None], 5);
        let good = delta(&open, &held([item(40, 1), None, None], 0));
        let mut weighed = good.clone();
        weighed.weight = Some(10);
        let mut added = good.clone();
        added.changes = vec![(4, SlotChange::Added(InventorySlot::empty(9, 3)))];
        let mut unchanged = good.clone();
        unchanged.changes.clear();
        for (inventories, message) in [
            (vec![weighed], "carries no weight"),
            (vec![added], "changes known slots"),
            (vec![unchanged], "changes known slots"),
            (vec![good.clone(), good], "duplicate live inventory delta"),
        ] {
            let (mut after_cache, mut after_exports) = (cache.clone(), exports.clone());
            let error = compose_live_pawn_bag_update(
                &mut after_cache,
                &mut after_exports,
                &plan,
                KEY,
                None,
                &[],
                None,
                &inventories,
                order,
            )
            .unwrap_err();
            assert!(error.to_string().contains(message), "{error}");
            assert_eq!(
                after_cache.subobject_guid(KEY, 1).unwrap(),
                cache.clone().subobject_guid(KEY, 1).unwrap()
            );
            assert_eq!(
                after_exports.bunch_exports().count(),
                exports.bunch_exports().count()
            );
        }
    }
}
