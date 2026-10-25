//! Experimental inventory weight cache, `C016` restore enablement, and
//! `RecalculateWeight` / `GetItemWeight` rules (F-933; U-075 resolved).
//!
//! Each helper models a bounded static rule from `3.0.31` server `25793607`:
//! - `UIcarusContainerManagerRecorderComponent` (`C016`) recording
//!   `bInventoryWantsTick` (`0x1416f0b50`), nulling unreferenced container
//!   slots during `StitchingOperation` (`0x1416f8490`), and restoring each
//!   saved container via `CreateInventoryInIndex` (`0x1416d38a0`),
//!   `AActor::FinishAddComponent` (`0x143338b80`), `UInventory::Initialise`
//!   (`0x1419750b0`), `SetComponentTickEnabled(bInventoryWantsTick)` and
//!   `UInventory::SetItem` (`0x141983140`) inside `UpdateOwnerToDatabaseState`
//!   (`0x1416fccb0`);
//! - `UInventory::GetStatContainerOwner` (`0x1419704e0`),
//!   `UInventory::RecalculateWeight` (`0x14197db50`) and
//!   `UInventoryItemLibrary::GetItemWeight` (`0x1416e0460`);
//! - initial actor-channel `CurrentWeight` (`+0xe8`, handle `4`) omission when
//!   equal to the `UInventory` CDO (`0`).
//!
//! Packaged data-table row meanings outside explicit caller inputs and live
//! connected-client execution remain outside this module.

/// `ENetRole::ROLE_Authority` (`3`), required (unsigned `>= 3`) by
/// `UInventory::Initialise` (`0x1419750b0`) and `UInventory::TickComponent`
/// (`0x141987d90`) (F-933).
pub const AUTHORITY_LOCAL_ROLE: u8 = 3;

/// `D_Stats` index (`0x5c8` / `1480`, `Weight_+`) queried on the item stat
/// container in `UInventoryItemLibrary::GetItemWeight` (`0x1416e04b4`).
pub const WEIGHT_STAT_INDEX: u16 = 0x5c8;

/// `D_TagQueries` row index (`0x16c` / `364`,
/// `Any_Weighable_Resource_Container`) tested in
/// `UInventoryItemLibrary::GetItemWeight` (`0x1416e04d5`).
pub const WEIGHABLE_RESOURCE_CONTAINER_QUERY_ROW: u16 = 0x16c;

/// `D_TagQueries` row index (`0xee` / `238`, `Any_Bag`) tested when binding
/// child bag inventories in `UInventory::OnContainerManagerUpdated`
/// (`0x141978220`).
pub const ANY_BAG_QUERY_ROW: u16 = 0xee;

/// `EDynamicItemProperties` discriminant (`0x0a` / `10`) added to unit weight
/// when `Any_Weighable_Resource_Container` matches (`0x1416e04f7`).
pub const DYNAMIC_PROP_FILLABLE_UNITS: u8 = 0x0a;

/// `EDynamicItemProperties` discriminant (`0x07` / `7`, `ItemableStack`)
/// required by `UInventory::RecalculateWeight` (`0x14197dcf8`) and multiplied
/// in `UInventoryItemLibrary::GetItemWeight` (`0x1416e0533`).
pub const DYNAMIC_PROP_ITEMABLE_STACK: u8 = 0x07;

/// `EDynamicItemProperties` discriminant (`0x0d` / `13`,
/// `AssociatedInventoryId`) scanned in `C016::StitchingOperation`
/// (`0x1416f8490`) and `UInventory::OnContainerManagerUpdated` (`0x1419782f4`).
pub const DYNAMIC_PROP_ASSOCIATED_INVENTORY_ID: u8 = 0x0d;

/// IEEE-754 binary32 bits (`0xbc23_d70a`, `-0.01f32`) loaded from `.rdata`
/// (`0x144a32cbc`) in `UInventoryItemLibrary::GetItemWeight` (`0x1416e05e8`).
pub const CARRY_WEIGHT_STAT_SCALE_BITS: u32 = 0xbc23_d70a;

/// Two-byte suppression word (`0x0101`) written at `UInventory + 0x331` around
/// `ManuallyPlaceItem` in `UInventory::SetItem` (`0x141983140`), leaving
/// `bDirtyWeight` at `UInventory + 0x330` untouched.
pub const SET_ITEM_SUPPRESSION_FLAGS: u16 = 0x0101;

/// `UInventory` class-default `CurrentWeight` (`0` at `+0xe8`), omitted on
/// initial actor-channel open when unchanged.
pub const DEFAULT_CDO_CURRENT_WEIGHT: i32 = 0;

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
    /// is at least 3 (unsigned, F-933). Return the new weight and clear dirty
    /// after calculation; otherwise preserve the cache and dirty state without
    /// calling `recalculate`. `None` means the actor owner is absent.
    pub fn tick(
        &mut self,
        owner_local_role: Option<u8>,
        recalculate: impl FnOnce() -> i32,
    ) -> Option<i32> {
        if !self.dirty || !owner_local_role.is_some_and(|role| role >= AUTHORITY_LOCAL_ROLE) {
            return None;
        }
        let weight = recalculate();
        self.cached_weight = weight;
        self.dirty = false;
        Some(weight)
    }
}

/// Which actor supplies the `UIcarusStatContainer` consulted by
/// `UInventory::GetStatContainerOwner` (`0x1419704e0`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum StatContainerOwnerSource {
    /// `OwnerPrivate` (`+0xa0`) carries `UIcarusStatContainer`.
    DirectActorOwner,
    /// `OwnerPrivate` lacks `UIcarusStatContainer` (for example
    /// `AInventoryContainerManager`, whose constructor `0x1416c39e0` destroys
    /// inherited `StatContainer` `+0x240`), and bound `OwningInventory`
    /// (`+0x3a0`) has an actor owner carrying `UIcarusStatContainer`.
    OwningInventoryActorOwner,
}

/// Resolves `UInventory::GetStatContainerOwner` (`0x1419704e0`).
pub const fn resolve_stat_container_owner(
    owner_has_stat_container: bool,
    owning_inventory_owner_has_stat_container: bool,
) -> Option<StatContainerOwnerSource> {
    if owner_has_stat_container {
        Some(StatContainerOwnerSource::DirectActorOwner)
    } else if owning_inventory_owner_has_stat_container {
        Some(StatContainerOwnerSource::OwningInventoryActorOwner)
    } else {
        None
    }
}

/// Evaluates the top-of-function owner gate in `UInventory::RecalculateWeight`
/// (`0x14197db73..0x14197dca7`): `OwnerPrivate` (`+0xa0`) must be valid and
/// not pending kill; `AInventoryContainerManager` owners bypass the
/// `UIcarusStatContainer` requirement (`0x14197dbff..0x14197dc3c`), while all
/// other owners abort early unless `OwnerPrivate` has a valid
/// `UIcarusStatContainer`.
pub const fn recalculate_weight_owner_eligible(
    has_valid_owner: bool,
    owner_is_container_manager: bool,
    owner_has_valid_stat_container: bool,
) -> bool {
    if !has_valid_owner {
        return false;
    }
    owner_is_container_manager || owner_has_valid_stat_container
}

/// Caller-supplied inputs for one slot's weight evaluation in
/// `UInventoryItemLibrary::GetItemWeight` (`0x1416e0460`) and
/// `UInventory::RecalculateWeight` (`0x14197db50`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ItemWeightInput {
    /// Whether `FItemData::GetItemableData` (`0x1413be310`) is non-null (and,
    /// in `RecalculateWeight`, `UInventoryItemLibrary::GetItemableData`
    /// `0x1416e0650` succeeds).
    pub has_valid_itemable: bool,
    /// `FStatContainer::GetStat(0x5c8)` (`Weight_+`) from
    /// `GetItemStatContainer(NULL, Item, false, true)` (`0x1416de990`).
    pub weight_stat: i32,
    /// Whether `ItemMatchesQuery(Item, 0x16c)`
    /// (`Any_Weighable_Resource_Container`) returned `true`.
    pub matches_weighable_resource_container: bool,
    /// First `DynamicData` entry with property `0x0a` (`10`), if present.
    pub fillable_units: Option<i32>,
    /// First `DynamicData` entry with property `0x07` (`7`, `ItemableStack`),
    /// if present.
    pub itemable_stack: Option<i32>,
    /// Sum of matching `D_ItemWeightStatQueries` stats from the resolved
    /// `StatContainerOwner` (`0` when `StatContainerOwner` is absent or lacks
    /// `UIcarusStatContainer`).
    pub matching_carry_weight_stat_pct: i32,
}

fn cvttss2si_i32(value: f32) -> i32 {
    if value.is_nan() || value < -2_147_483_648.0 || value >= 2_147_483_648.0 {
        i32::MIN
    } else {
        value as i32
    }
}

/// Computes `UInventoryItemLibrary::GetItemWeight` (`0x1416e0460..0x1416e0648`):
/// adds `fillable_units` when `matches_weighable_resource_container` is set,
/// returns `0` when unit weight `<= 0`, defaults missing `ItemableStack` (`7`)
/// to `1`, applies `CVTTSS2SI(((stat_pct as f32) * -0.01f32) * (unit as f32))`
/// subtraction clamped at `>= 1`, and multiplies by stack (`IMUL`).
pub fn item_weight(input: &ItemWeightInput) -> i32 {
    if !input.has_valid_itemable {
        return 0;
    }
    let mut unit_weight = input.weight_stat;
    if input.matches_weighable_resource_container {
        unit_weight = unit_weight.wrapping_add(input.fillable_units.unwrap_or(0));
    }
    if unit_weight <= 0 {
        return 0;
    }
    let stack = input.itemable_stack.unwrap_or(1);
    let scale = f32::from_bits(CARRY_WEIGHT_STAT_SCALE_BITS);
    let delta = cvttss2si_i32(
        ((input.matching_carry_weight_stat_pct as f32) * scale) * (unit_weight as f32),
    );
    let adjusted_unit = unit_weight.wrapping_sub(delta).max(1);
    adjusted_unit.wrapping_mul(stack)
}

/// Computes one slot's contribution inside `UInventory::RecalculateWeight`
/// (`0x14197dcd0..0x14197dec8`): requires `GetItemPropertyValue(7, ...)`
/// (`ItemableStack`) to report `EDataValid::DataValid` (`is_some()`) before
/// calling `GetItemWeight` (`0x1416e0460`), skipping slots that omit property `7`.
pub fn recalculate_slot_weight(input: &ItemWeightInput) -> i32 {
    if !input.has_valid_itemable || input.itemable_stack.is_none() {
        return 0;
    }
    item_weight(input)
}

/// Outcome of `UInventory::RecalculateWeight` (`0x14197db50`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecalculateWeightOutcome {
    pub current_weight: i32,
    pub weight_changed: bool,
    /// Whether `OwningInventory->bDirtyWeight` (`+0x330`) is set to `1`
    /// (`0x14197e219..0x14197e237`).
    pub mark_owning_inventory_dirty: bool,
}

/// Evaluates `UInventory::RecalculateWeight` (`0x14197db50`): checks owner
/// eligibility, sums `recalculate_slot_weight` across slots plus bound child
/// `BagInventories` (`+0x350`, `0x14197e0b0..0x14197e0d4`), updates
/// `CurrentWeight` (`+0xe8`) when changed, and propagates `bDirtyWeight = 1`
/// to a bound `OwningInventory` (`+0x3a0`) on change.
pub fn recalculate_inventory_weight(
    previous_weight: i32,
    has_valid_owner: bool,
    owner_is_container_manager: bool,
    owner_has_valid_stat_container: bool,
    has_owning_inventory: bool,
    slots: &[ItemWeightInput],
    child_bag_weights: &[i32],
) -> Option<RecalculateWeightOutcome> {
    if !recalculate_weight_owner_eligible(
        has_valid_owner,
        owner_is_container_manager,
        owner_has_valid_stat_container,
    ) {
        return None;
    }
    let mut total: i32 = 0;
    for slot in slots {
        total = total.wrapping_add(recalculate_slot_weight(slot));
    }
    for &bag_weight in child_bag_weights {
        total = total.wrapping_add(bag_weight);
    }
    let weight_changed = total != previous_weight;
    Some(RecalculateWeightOutcome {
        current_weight: total,
        weight_changed,
        mark_owning_inventory_dirty: weight_changed && has_owning_inventory,
    })
}

/// Initial actor-channel `CurrentWeight` (`+0xe8`, rep-layout handle `4`)
/// emission against the `UInventory` CDO (`0`): `None` omits handle `4` when
/// `current_weight == 0`.
pub const fn initial_open_current_weight(current_weight: i32) -> Option<i32> {
    if current_weight == DEFAULT_CDO_CURRENT_WEIGHT {
        None
    } else {
        Some(current_weight)
    }
}

/// Records `FSavedInventoryContainerData::bInventoryWantsTick` (`+0x28`) from
/// `UActorComponent::IsComponentTickEnabled()` in `C016::RecordOwnerState`
/// (`0x1416f0b50`).
pub const fn record_container_wants_tick(component_tick_enabled: bool) -> bool {
    component_tick_enabled
}

/// Returns the indices in `0..inventories_len` unreferenced by any saved
/// `AssociatedInventoryId` (`13`), which `C016::StitchingOperation`
/// (`0x1416f8490`) passes to `AInventoryContainerManager::RemoveInventory`
/// (nulling `Inventories[index]` without shrinking `Inventories.Num()`).
pub fn unreferenced_container_indices(
    inventories_len: usize,
    referenced_associated_ids: &[i32],
) -> Vec<usize> {
    (0..inventories_len)
        .filter(|&index| {
            i32::try_from(index)
                .ok()
                .is_none_or(|id| !referenced_associated_ids.contains(&id))
        })
        .collect()
}

/// Lifecycle state of one `UInventory` restored into
/// `AInventoryContainerManager::Inventories` by
/// `C016::UpdateOwnerToDatabaseState` (`0x1416fccb0`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RestoredContainerInventory {
    owner_local_role: Option<u8>,
    registered: bool,
    tick_enabled: bool,
    weight: InventoryWeightState,
}

impl RestoredContainerInventory {
    /// Models `CreateInventoryInIndex` (`0x1416d38a0`) +
    /// `AActor::FinishAddComponent` (`0x143338b80`, `bAutoRegister = 1`) +
    /// `UInventory::Initialise` (`0x1419750b0`, sets `bDirtyWeight = 1` on
    /// authority and disables tick) + `SetComponentTickEnabled(wants_tick)`
    /// (`0x1416fcec0..0x1416fcec7`) + `UInventory::SetItem` (`0x141983140`,
    /// preserving `bDirtyWeight`).
    pub fn from_c016_restore(owner_local_role: Option<u8>, wants_tick: bool) -> Self {
        let mut weight = InventoryWeightState::new(DEFAULT_CDO_CURRENT_WEIGHT);
        if owner_local_role.is_some_and(|role| role >= AUTHORITY_LOCAL_ROLE) {
            weight.mark_dirty();
        }
        Self {
            owner_local_role,
            registered: true,
            tick_enabled: wants_tick,
            weight,
        }
    }

    pub fn is_registered(&self) -> bool {
        self.registered
    }

    pub fn is_tick_enabled(&self) -> bool {
        self.tick_enabled
    }

    pub fn weight_state(&self) -> &InventoryWeightState {
        &self.weight
    }

    /// Runs `UInventory::TickComponent` (`0x141987d90`) only when registered
    /// and component tick is enabled; otherwise preserves `cached_weight` (`0`
    /// after restore) and `bDirtyWeight` without calling `recalculate`.
    pub fn tick_if_enabled(&mut self, recalculate: impl FnOnce() -> i32) -> Option<i32> {
        if !self.registered || !self.tick_enabled {
            return None;
        }
        self.weight.tick(self.owner_local_role, recalculate)
    }

    /// Handle `4` (`CurrentWeight`) emitted on initial actor-channel open.
    pub fn initial_open_weight(&self) -> Option<i32> {
        initial_open_current_weight(self.weight.cached_weight())
    }
}

/// Restores the `Owner->Inventories` array (`NumInventories` `None`-initialized
/// slots) in `C016::UpdateOwnerToDatabaseState` (`0x1416fccb0`), skipping
/// out-of-bounds `InventoryIndex` entries (`index < 0 || index >= count`).
pub fn restore_container_manager_inventories(
    num_inventories: i32,
    owner_local_role: Option<u8>,
    saved_entries: &[(i32, bool)],
) -> Vec<Option<RestoredContainerInventory>> {
    let len = usize::try_from(num_inventories.max(0)).unwrap_or(0);
    let mut inventories = vec![None; len];
    for &(index, wants_tick) in saved_entries {
        if let Some(slot) = usize::try_from(index)
            .ok()
            .and_then(|idx| inventories.get_mut(idx))
        {
            *slot = Some(RestoredContainerInventory::from_c016_restore(
                owner_local_role,
                wants_tick,
            ));
        }
    }
    inventories
}
