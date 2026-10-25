//! Experimental `UModifierStateComponent` lifecycle, stat-scaling and callback rules.
//!
//! The existing lifecycle rules are pure rules over caller-supplied component and
//! owner state, statically recovered from server `25621760` and client `25793767` (F-930)
//! (`UModifierStateComponent` primary vftable `0x144a263a8` server / `0x144b94c08`
//! client, `Deactivate`, `Activate`, `UpdateStats`, per-stat scaling helper
//! `0x1416b6710` / `0x1416ce2f0`, `GetModifierStateData`, `BeginPlay`, `EndPlay`,
//! `InitComponent` / `InitComponent_Implementation`,
//! `ModifierApplied_Implementation`, `ModifierRemoved_Implementation`,
//! `OnStatContainerUpdated`, and `UIcarusFunctionLibrary::FixModifierActivation`).
//! Blueprint `ProcessEvent` overrides, packaged data-table row meanings (F-660)
//! and live original-process execution remain outside this module.
//!
//! The local `RemoveModifierState` selection model (F-937; server `25621760`,
//! client `25793767`) consumes an ordered snapshot of valid, non-null candidates
//! and supplied opaque comparison results.
//! It describes a request and a local success-after-return flag, never destruction.
//! Enumeration, owner lookup, comparison semantics and subclass effects are excluded.
//!
//! The optional comparator model below describes physical caller-local operation
//! order only. Its callbacks retain opaque operands, result tokens and effects;
//! it is independent of the selection model's supplied comparison bools.

/// `EStatSources` discriminant (`3`) passed by `UModifierStateComponent::Deactivate`
/// and `UModifierStateComponent::UpdateStats` to `UIcarusStatContainer::RemoveStats_BP`
/// and `UIcarusStatContainer::AddStats` alongside `*(i32*)(this + 0xf8)` (F-930).
pub const MODIFIER_STAT_SOURCE_KIND: u8 = 3;

/// `ENetRole::ROLE_Authority` (`3`) required by `UModifierStateComponent::BeginPlay`,
/// the authority teardown block of `UModifierStateComponent::EndPlay`, and the
/// `+0x130` multicast broadcast in `OnStatContainerUpdated` (F-930).
pub const MODIFIER_AUTHORITY_NET_ROLE: i32 = 3;

/// Byte value at `this + 0x319` (`3`) that triggers
/// `UIcarusFunctionLibrary::FixModifierActivation(OwnerPrivate, &ModifierRowHandle)`
/// inside the authority `EndPlay(0)` block (F-930).
pub const MODIFIER_FIX_ACTIVATION_STATE: u8 = 3;

/// `EStats` sentinel (`0xff` / `255`) returned by `FStatsEnum::ToCppEnum`
/// (`0x140b032f0` server / `0x140b03fb0` client) that skips effectiveness
/// scaling in `0x1416b6710` server / `0x1416ce2f0` client.
pub const MODIFIER_INVALID_STAT_ENUM: u8 = 0xff;

/// IEEE-754 binary32 bits (`0x3c23_d70a`, `0.01f32`) loaded from `.rdata`
/// (`0x1447a5df8` server) in `0x1416b6710` server / `0x1416ce2f0` client.
pub const MODIFIER_STAT_SCALE_CENTI_BITS: u32 = 0x3c23_d70a;

/// IEEE-754 binary32 bits (`0x3dcc_cccd`, `0.1f32`) passed as `InRate` for the
/// looping `HighFrequencyTick` timer in `InitComponent_Implementation`
/// (`0x1416e88f0` server / `0x1416fa330` client).
pub const MODIFIER_HIGH_FREQUENCY_TICK_RATE_BITS: u32 = 0x3dcc_cccd;

/// Default `InFirstDelay` (`-1.0f32`, `0xbf80_0000`) passed to
/// `FTimerManager::InternalSetTimer` in `InitComponent_Implementation` and
/// `OnStatContainerUpdated`.
pub const MODIFIER_TIMER_DEFAULT_FIRST_DELAY: f32 = -1.0;

/// Packed `FAttachmentTransformRules` (`0x0002_0202`: `KeepRelative` = `2` for
/// Location, Rotation and Scale, `bWeldSimulatedBodies = false`) passed to
/// `USceneComponent::AttachToComponent` in `InitComponent_Implementation`.
pub const MODIFIER_SCENE_ATTACH_KEEP_RELATIVE_PACKED: u32 = 0x0002_0202;

/// Selected `UModifierStateComponent` primary vftable (`0x144a263a8` server /
/// `0x144b94c08` client) slot bindings shared across server `25621760` and
/// client `25793767` (F-930).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierPrimaryVftableSlot {
    /// `+0x220`: `UObject::ProcessEvent` (`0x14217c5f0` server / `0x142145fd0` client).
    UObjectProcessEvent,
    /// `+0x270`: `UActorComponent::OnRep_IsActive` (`0x143468110` server / `0x1436fd610` client).
    ActorComponentOnRepIsActive,
    /// `+0x278`: `UModifierStateComponent::Activate` (`0x1416ccb60` server / `0x1416df050` client).
    ModifierActivate,
    /// `+0x280`: `UModifierStateComponent::Deactivate` (`0x1416d48e0` server / `0x1416e6960` client).
    ModifierDeactivate,
    /// `+0x288`: `UActorComponent::SetActive` (`0x14346c530` server / `0x1437017d0` client).
    ActorComponentSetActive,
    /// `+0x338`: `UModifierStateComponent::BeginPlay` (`0x1416cfde0` server / `0x1416e2220` client).
    ModifierBeginPlay,
    /// `+0x340`: `UModifierStateComponent::EndPlay` (`0x1416d5af0` server / `0x1416e7b70` client).
    ModifierEndPlay,
    /// `+0x3c0`: `UActorComponent::DestroyComponent` (`0x14345e1e0` server / `0x1436f3af0` client).
    ActorComponentDestroyComponent,
    /// `+0x400`: ICF-folded `false` stub (`32 c0 c3`: `0x14089a160` server / `0x1408cb520` client).
    IcfReturnFalseStub400,
    /// `+0x408`: `UModifierStateComponent::UpdateStats` (`0x1416fea90` server / `0x14170f320` client).
    ModifierUpdateStats,
    /// `+0x410`: `UModifierStateComponent::ModifierApplied_Implementation`
    /// (`0x1416eae50` server / `0x1416fca50` client).
    ModifierAppliedImplementation,
    /// `+0x418`: `UModifierStateComponent::ModifierRemoved_Implementation`
    /// (`0x1416eafd0` server / `0x1416fcb90` client).
    ModifierRemovedImplementation,
    /// `+0x420`: ICF-folded void no-op stub (`c2 00 00`: `0x140898810` server / `0x1408c9710` client).
    IcfVoidNoOpStub420,
    /// `+0x428`: ICF-folded `true` stub (`b0 01 c3`: `0x140899f70` server / `0x1408c9810` client).
    IcfReturnTrueStub428,
    /// `+0x430`: `UModifierStateComponent::InitComponent_Implementation`
    /// (`0x1416e88f0` server / `0x1416fa330` client).
    ModifierInitComponentImplementation,
    /// `+0x438`: ICF-folded `true` stub (`b0 01 c3`: `0x140899f70` server / `0x1408c9810` client).
    IcfReturnTrueStub438,
}

/// Resolves one qualified `UModifierStateComponent` primary vftable byte offset
/// (`0x144a263a8` server / `0x144b94c08` client) (F-930).
pub const fn resolve_modifier_primary_vftable_slot(
    byte_offset: u16,
) -> Option<ModifierPrimaryVftableSlot> {
    match byte_offset {
        0x220 => Some(ModifierPrimaryVftableSlot::UObjectProcessEvent),
        0x270 => Some(ModifierPrimaryVftableSlot::ActorComponentOnRepIsActive),
        0x278 => Some(ModifierPrimaryVftableSlot::ModifierActivate),
        0x280 => Some(ModifierPrimaryVftableSlot::ModifierDeactivate),
        0x288 => Some(ModifierPrimaryVftableSlot::ActorComponentSetActive),
        0x338 => Some(ModifierPrimaryVftableSlot::ModifierBeginPlay),
        0x340 => Some(ModifierPrimaryVftableSlot::ModifierEndPlay),
        0x3c0 => Some(ModifierPrimaryVftableSlot::ActorComponentDestroyComponent),
        0x400 => Some(ModifierPrimaryVftableSlot::IcfReturnFalseStub400),
        0x408 => Some(ModifierPrimaryVftableSlot::ModifierUpdateStats),
        0x410 => Some(ModifierPrimaryVftableSlot::ModifierAppliedImplementation),
        0x418 => Some(ModifierPrimaryVftableSlot::ModifierRemovedImplementation),
        0x420 => Some(ModifierPrimaryVftableSlot::IcfVoidNoOpStub420),
        0x428 => Some(ModifierPrimaryVftableSlot::IcfReturnTrueStub428),
        0x430 => Some(ModifierPrimaryVftableSlot::ModifierInitComponentImplementation),
        0x438 => Some(ModifierPrimaryVftableSlot::IcfReturnTrueStub438),
        _ => None,
    }
}

/// Outcome of `UModifierStateComponent::Deactivate` (`vftable + 0x280`:
/// `0x1416d48e0` server / `0x1416e6960` client) (F-930).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModifierDeactivateOutcome {
    /// `UActorComponent::Deactivate(this)` (`0x14345e0c0` server / `0x1436f39d0`
    /// client) always runs before owner lookup.
    pub run_base_deactivate: bool,
    /// `Some((MODIFIER_STAT_SOURCE_KIND, modifier_uid_f8))` passed to
    /// `UIcarusStatContainer::RemoveStats_BP` (`0x1417f1430` server /
    /// `0x1417fd740` client) iff `OwnerPrivate` (`+0xa0`) is non-null and its
    /// `GetComponentByClass(UIcarusStatContainer::StaticClass())` passes the
    /// `UStructBaseChain` check.
    pub remove_stats_source_and_id: Option<(u8, i32)>,
}

/// Evaluates `UModifierStateComponent::Deactivate` (`vftable + 0x280`:
/// `0x1416d48e0` server / `0x1416e6960` client) (F-930).
pub const fn evaluate_modifier_deactivate(
    owner_present: bool,
    owner_has_valid_stat_container: bool,
    modifier_uid_f8: i32,
) -> ModifierDeactivateOutcome {
    let remove_stats_source_and_id = if owner_present && owner_has_valid_stat_container {
        Some((MODIFIER_STAT_SOURCE_KIND, modifier_uid_f8))
    } else {
        None
    };
    ModifierDeactivateOutcome {
        run_base_deactivate: true,
        remove_stats_source_and_id,
    }
}

/// Outcome of `UModifierStateComponent::Activate(bool bReset)` (`vftable + 0x278`:
/// `0x1416ccb60` server / `0x1416df050` client) (F-930).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierActivateOutcome {
    /// `(*(u8*)(this + 0x8a) & 1) != 0` before `UActorComponent::Activate(this, bReset)`,
    /// or `OwnerPrivate` (`+0xa0`) is `nullptr`: runs `UActorComponent::Activate`
    /// (`0x143457bc0` server / `0x1436ed530` client) and returns.
    AlreadyActiveOrMissingOwner { forwarded_b_reset: bool },
    /// `OwnerPrivate` is non-null, but `GetComponentByClass(UIcarusStatContainer::StaticClass())`
    /// returned `nullptr` or failed `UStructBaseChain` (`stat_container = nullptr`),
    /// and `Activate` unconditionally passes `nullptr` to
    /// `FSparseDelegateStorage::Add` and writes `*(u8*)(nullptr + 0xb3) |= added`.
    NullStatContainerSparseDelegateCrash { forwarded_b_reset: bool },
    /// Binds `"OnPreStatContainerUpdateComplete"` -> `(this, "OnStatContainerUpdated")`
    /// in `FSparseDelegateStorage` (`0x14216a930` server / `0x142135b90` client),
    /// ORs `sparse_delegate_added` into `*(u8*)(stat_container + 0xb3)`, and
    /// dispatches `this->vtable[+0x408](this)` (`UpdateStats`).
    BoundDelegateAndUpdatedStats {
        forwarded_b_reset: bool,
        set_stat_container_flag_b3: bool,
        dispatch_update_stats: bool,
    },
}

/// Evaluates `UModifierStateComponent::Activate(bool bReset)` (`vftable + 0x278`:
/// `0x1416ccb60` server / `0x1416df050` client) (F-930).
pub const fn evaluate_modifier_activate(
    b_was_active: bool,
    b_reset: bool,
    owner_present: bool,
    owner_has_valid_stat_container: bool,
    sparse_delegate_added: bool,
) -> ModifierActivateOutcome {
    if b_was_active || !owner_present {
        return ModifierActivateOutcome::AlreadyActiveOrMissingOwner {
            forwarded_b_reset: b_reset,
        };
    }
    if !owner_has_valid_stat_container {
        return ModifierActivateOutcome::NullStatContainerSparseDelegateCrash {
            forwarded_b_reset: b_reset,
        };
    }
    ModifierActivateOutcome::BoundDelegateAndUpdatedStats {
        forwarded_b_reset: b_reset,
        set_stat_container_flag_b3: sparse_delegate_added,
        dispatch_update_stats: true,
    }
}

/// Outcome of `UModifierStateComponent::UpdateStats` (`vftable + 0x408`:
/// `0x1416fea90` server / `0x14170f320` client) (F-930).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierUpdateStatsOutcome {
    /// `(*(u8*)(this + 0x8a) & 1) == 0`: returns without querying owner or stats.
    InactiveNoOp,
    /// `OwnerPrivate` (`+0xa0`) lacks a valid `UIcarusStatContainer`: returns
    /// without removing or adding stats.
    MissingStatContainerNoOp,
    /// Calls `UIcarusStatContainer::RemoveStats_BP(stat_container, 3, modifier_uid_f8)`,
    /// converts `granted_stat_entry_count` entries from
    /// `GetModifierStateData().GrantedStats` (`FModifierStateData + 0xa0`) via
    /// `0x1416b6710` / `0x1416ce2f0` into `TMap<EStats, int32>`, and calls
    /// `UIcarusStatContainer::AddStats(stat_container, FStatSource { 3, modifier_uid_f8 }, map)`
    /// (`0x14178c9f0` server / `0x1417998a0` client).
    ReplacedModifierStats {
        removed_source_and_id: (u8, i32),
        added_source_and_id: (u8, i32),
        converted_stat_entries: usize,
    },
}

/// Evaluates `UModifierStateComponent::UpdateStats` (`vftable + 0x408`:
/// `0x1416fea90` server / `0x14170f320` client) (F-930).
pub const fn evaluate_modifier_update_stats(
    b_is_active: bool,
    owner_has_valid_stat_container: bool,
    modifier_uid_f8: i32,
    granted_stat_entry_count: usize,
) -> ModifierUpdateStatsOutcome {
    if !b_is_active {
        return ModifierUpdateStatsOutcome::InactiveNoOp;
    }
    if !owner_has_valid_stat_container {
        return ModifierUpdateStatsOutcome::MissingStatContainerNoOp;
    }
    let source = (MODIFIER_STAT_SOURCE_KIND, modifier_uid_f8);
    ModifierUpdateStatsOutcome::ReplacedModifierStats {
        removed_source_and_id: source,
        added_source_and_id: source,
        converted_stat_entries: granted_stat_entry_count,
    }
}

/// Outcome of `UModifierStateComponent::BeginPlay` (`vftable + 0x338`:
/// `0x1416cfde0` server / `0x1416e2220` client) (F-930).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierBeginPlayOutcome {
    /// `GetOwnerRole(this) != 3`: runs `UActorComponent::BeginPlay` (`0x143459b20`
    /// server / `0x1436ef470` client) and returns.
    NonAuthorityBaseOnly,
    /// `GetOwnerRole(this) == 3` (`ROLE_Authority`): runs `UActorComponent::BeginPlay`,
    /// move-assigns `GetModifierStateData(this)` into `this + 0x140`, calls
    /// `ModifierApplied()` (`0x141c9d380` server / `0x141c8f440` client) iff
    /// `*(u8*)(this + 0x3b0) == 0`, and calls `InitComponent()` (`0x141c9d340`
    /// server / `0x141c8f400` client).
    AuthorityInitialized {
        cache_modifier_state_data_140: bool,
        call_modifier_applied: bool,
        call_init_component: bool,
    },
}

/// Evaluates `UModifierStateComponent::BeginPlay` (`vftable + 0x338`:
/// `0x1416cfde0` server / `0x1416e2220` client) (F-930).
pub const fn evaluate_modifier_begin_play(
    owner_role: i32,
    suppress_modifier_applied_3b0: bool,
) -> ModifierBeginPlayOutcome {
    if owner_role != MODIFIER_AUTHORITY_NET_ROLE {
        return ModifierBeginPlayOutcome::NonAuthorityBaseOnly;
    }
    ModifierBeginPlayOutcome::AuthorityInitialized {
        cache_modifier_state_data_140: true,
        call_modifier_applied: !suppress_modifier_applied_3b0,
        call_init_component: true,
    }
}

/// Which `FTimerManager` receives timer registration or clearing inside
/// `UModifierStateComponent` (`EndPlay`, `InitComponent_Implementation`, and
/// `OnStatContainerUpdated`) (F-930).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierTimerClearTarget {
    /// Non-authority role, or both `WorldPrivate` (`this + 0xa8`) and
    /// `GetWorld_Uncached(this)` returned `nullptr`.
    None,
    /// `World->OwningGameInstance` (`World + 0x180`) is non-null: selects
    /// `*(FTimerManager**)(OwningGameInstance + 0xd0)`.
    GameInstanceTimerManager,
    /// `World->OwningGameInstance` (`World + 0x180`) is `nullptr`: selects
    /// `*(FTimerManager**)(World + 0x368)`.
    WorldTimerManager,
}

/// Inputs for `UModifierStateComponent::EndPlay(EEndPlayReason::Type)`
/// (`vftable + 0x340`: `0x1416d5af0` server / `0x1416e7b70` client) (F-930).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModifierEndPlayInputs {
    pub end_play_reason: u8,
    pub owner_role: i32,
    /// Whether `WorldPrivate` (`this + 0xa8`) or `GetWorld_Uncached(this)` is non-null.
    pub has_world: bool,
    /// Whether `World->OwningGameInstance` (`World + 0x180`) is non-null.
    pub world_has_owning_game_instance: bool,
    /// Byte at `this + 0x319` (`3` triggers `UIcarusFunctionLibrary::FixModifierActivation`).
    pub activation_state_319: u8,
    /// Whether `*(UActorComponent**)(this + 0x100)` is non-null.
    pub has_attached_component_100: bool,
}

/// Outcome of `UModifierStateComponent::EndPlay(EEndPlayReason::Type)`
/// (`vftable + 0x340`: `0x1416d5af0` server / `0x1416e7b70` client) (F-930).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierEndPlayOutcome {
    /// `end_play_reason != 0`: calls `UActorComponent::EndPlay` (`0x14345ed50`
    /// server / `0x1436f4790` client), writes `*(u64*)(this + 0x3c0) = 0`, and
    /// returns without running modifier removal, deactivation or notifications.
    NonZeroReasonBaseAndClear3c0Only,
    /// `end_play_reason == 0` (`EEndPlayReason::Destroyed`): clears `+0x3c0`,
    /// runs the authority removal/timer/deactivate/`FixModifierActivation` block
    /// iff `owner_role == 3`, and then unconditionally destroys any component at
    /// `+0x100` (`comp->vtable[+0x3c0](comp, false)`), calls
    /// `IModifiableInterface::Execute_OnModifiersUpdated(OwnerPrivate, this, true)`
    /// (`0x141c9d220` server / `0x141c8f2e0` client), and broadcasts
    /// `OnModifierUpdated(this, true)` (`+0x12c`).
    ReasonZeroTeardown {
        clear_field_3c0: bool,
        call_modifier_removed: bool,
        timer_clear_target: ModifierTimerClearTarget,
        dispatch_virtual_deactivate: bool,
        call_fix_modifier_activation: bool,
        destroy_attached_component_100: bool,
        notify_owner_modifiable_interface_removed: bool,
        broadcast_on_modifier_updated_removed: bool,
    },
}

/// Evaluates `UModifierStateComponent::EndPlay(EEndPlayReason::Type)`
/// (`vftable + 0x340`: `0x1416d5af0` server / `0x1416e7b70` client) (F-930).
pub const fn evaluate_modifier_end_play(inputs: &ModifierEndPlayInputs) -> ModifierEndPlayOutcome {
    if inputs.end_play_reason != 0 {
        return ModifierEndPlayOutcome::NonZeroReasonBaseAndClear3c0Only;
    }
    let is_authority = inputs.owner_role == MODIFIER_AUTHORITY_NET_ROLE;
    let timer_clear_target = if !is_authority || !inputs.has_world {
        ModifierTimerClearTarget::None
    } else if inputs.world_has_owning_game_instance {
        ModifierTimerClearTarget::GameInstanceTimerManager
    } else {
        ModifierTimerClearTarget::WorldTimerManager
    };
    ModifierEndPlayOutcome::ReasonZeroTeardown {
        clear_field_3c0: true,
        call_modifier_removed: is_authority,
        timer_clear_target,
        dispatch_virtual_deactivate: is_authority,
        call_fix_modifier_activation: is_authority
            && inputs.activation_state_319 == MODIFIER_FIX_ACTIVATION_STATE,
        destroy_attached_component_100: inputs.has_attached_component_100,
        notify_owner_modifiable_interface_removed: true,
        broadcast_on_modifier_updated_removed: true,
    }
}

fn cvtss2si_rnear_i32(x: f32) -> i32 {
    let r = x.round_ties_even();
    if !r.is_finite() || r < (i32::MIN as f32) || r >= 2_147_483_648.0_f32 {
        i32::MIN
    } else {
        r as i32
    }
}

fn cvttss2si_i32(x: f32) -> i32 {
    let t = x.trunc();
    if !t.is_finite() || t < (i32::MIN as f32) || t >= 2_147_483_648.0_f32 {
        i32::MIN
    } else {
        t as i32
    }
}

/// Scales one `GrantedStats` entry `(FStatsEnum -> stat_enum, raw_value)` using
/// `*(i32*)(this + 0x128)` (`effectiveness_pct_128`), matching the `UpdateStats`
/// lambda helper (`0x1416b6710` server / `0x1416ce2f0` client, `223` B).
///
/// When `stat_enum == 0xff` (`MODIFIER_INVALID_STAT_ENUM`), scaling is skipped
/// and `(stat_enum, raw_value)` is returned unchanged. Otherwise executes the
/// exact SSE2 sequence (`((effectiveness_pct as f32) + 100.0f32) * (raw_value as f32)
/// * f32::from_bits(0x3c23d70a)` followed by MSVC's `-0.5f32 - 2.0f32 * abs`
/// `CVTSS2SI`/`SAR 1`/`NEG` `ceilf(|scaled|)` idiom, sign restoration and
/// `CVTTSS2SI`, rounding non-integer magnitudes away from zero).
pub fn scale_modifier_granted_stat_value(
    stat_enum: u8,
    raw_value: i32,
    effectiveness_pct_128: i32,
) -> (u8, i32) {
    if stat_enum == MODIFIER_INVALID_STAT_ENUM {
        return (stat_enum, raw_value);
    }
    let eff_plus_100 = (effectiveness_pct_128 as f32) + 100.0_f32;
    let scaled =
        (eff_plus_100 * (raw_value as f32)) * f32::from_bits(MODIFIER_STAT_SCALE_CENTI_BITS);
    let sgn = if scaled > 0.0_f32 {
        1.0_f32
    } else if scaled == 0.0_f32 {
        0.0_f32
    } else {
        // Matches `COMISS scaled, 0.0; JBE; JNC` (both `< 0.0` and unordered).
        -1.0_f32
    };
    let abs_scaled = f32::from_bits(scaled.to_bits() & 0x7fff_ffff);
    let twice_abs = abs_scaled + abs_scaled;
    let diff = -0.5_f32 - twice_abs;
    let rounded_neg_twice = cvtss2si_rnear_i32(diff);
    let rounded_abs_i32 = (rounded_neg_twice >> 1).wrapping_neg();
    let signed_rounded = (rounded_abs_i32 as f32) * sgn;
    (stat_enum, cvttss2si_i32(signed_rounded))
}

/// Source path selected by `UModifierStateComponent::GetModifierStateData`
/// (`0x1416e2e90` server / `0x1416f4960` client, `235` B).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierStateDataSource {
    /// `ModifierRowHandle.RowName` (`this + 0xbc`, `this + 0xc0`) is `NAME_None`
    /// (`ComparisonIndex == 0 && Number == 0`): default-constructs
    /// `FModifierStateData` directly into the return slot without calling
    /// `FModifierStatesRowHandle::GetStruct`.
    DefaultConstructedFromNoneRowName,
    /// `RowName != NAME_None` and `FModifierStatesRowHandle::GetStruct(this + 0xb4)`
    /// returned a non-null `const FModifierStateData*`: copy-constructs the
    /// return slot from that table row.
    CopyConstructedFromTableRow,
    /// `RowName != NAME_None`, `GetStruct(this + 0xb4)` returned `nullptr`:
    /// default-constructs a stack temporary `FModifierStateData`, copy-constructs
    /// the return slot from it, and destroys the temporary.
    CopyConstructedFromTemporaryDefault,
}

/// Evaluates `UModifierStateComponent::GetModifierStateData` (`0x1416e2e90`
/// server / `0x1416f4960` client).
pub const fn evaluate_modifier_get_state_data(
    row_name_is_none: bool,
    row_struct_found: bool,
) -> ModifierStateDataSource {
    if row_name_is_none {
        ModifierStateDataSource::DefaultConstructedFromNoneRowName
    } else if row_struct_found {
        ModifierStateDataSource::CopyConstructedFromTableRow
    } else {
        ModifierStateDataSource::CopyConstructedFromTemporaryDefault
    }
}

/// Scene-component construction step in
/// `UModifierStateComponent::InitComponent_Implementation` (`vftable + 0x430`:
/// `0x1416e88f0` server / `0x1416fa330` client, `1,136` B).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierSceneComponentInitAction {
    /// `FSoftObjectPath` at `this + 0x1b0` equals an empty `FSoftObjectPath`:
    /// skips construction, registration and attachment.
    SkippedEmptySoftPath,
    /// `this + 0x1b0` is non-empty, `TSoftClassPtr<USceneComponent>` (`this + 0x1a0`)
    /// resolved to `nullptr` or a `UClass` that is not a child of
    /// `USceneComponent`, and `InitComponent_Implementation` still passes
    /// `InClass = nullptr` unconditionally to `FStaticConstructObjectParameters` /
    /// `StaticConstructObject_Internal`.
    NullClassConstructCrash,
    /// Constructs the resolved `USceneComponent` subclass with
    /// `Outer = OwnerPrivate` (`+0xa0`), stores it at `this + 0x100`, calls
    /// `RegisterComponent()`, and attaches it to `OwnerPrivate->RootComponent`
    /// (`+0x130`) with `FAttachmentTransformRules` `0x0002_0202` (`KeepRelative`)
    /// and `SocketName = NAME_None`.
    ConstructRegisterAndAttachKeepRelative { attachment_rules_packed: u32 },
}

/// Inputs for `UModifierStateComponent::InitComponent_Implementation`
/// (`vftable + 0x430`: `0x1416e88f0` server / `0x1416fa330` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ModifierInitComponentInputs {
    /// Whether `FSoftObjectPath` at `this + 0x1b0` is non-empty.
    pub soft_scene_class_path_non_empty_1b0: bool,
    /// Whether `TSoftClassPtr<USceneComponent>` (`this + 0x1a0`) resolved to a
    /// valid `UClass` with `IsChildOf(USceneComponent::StaticClass())`.
    pub resolved_valid_scene_component_class: bool,
    /// `*(u8*)(this + 0x300) != 0` (`bHighFrequencyTick` flag in cached `FModifierStateData`).
    pub enable_high_frequency_tick_300: bool,
    /// Value read from `*(u32*)(World + 0x5a0)` when `+0x300 != 0`.
    pub world_field_5a0: u32,
    /// Whether `World->OwningGameInstance` (`World + 0x180`) is non-null.
    pub world_has_owning_game_instance: bool,
    /// `*(u8*)(this + 0x301) != 0` (immediate initial tick flag in cached `FModifierStateData`).
    pub tick_immediately_301: bool,
    /// Result of `CheckTickConditions(this)` (`0x141c9cad0` server / `0x141c8eb90` client).
    pub check_tick_conditions_passed: bool,
}

/// Looping timer registration parameters for `HighFrequencyTick` (`+0x3c0`) or
/// `AutoDestroy` (`+0x3a8`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModifierLoopingTimerSetup {
    pub timer_manager: ModifierTimerClearTarget,
    pub rate_seconds: f32,
    pub loop_timer: bool,
    pub first_delay_seconds: f32,
}

/// Outcome of `UModifierStateComponent::InitComponent_Implementation`
/// (`vftable + 0x430`: `0x1416e88f0` server / `0x1416fa330` client).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModifierInitComponentOutcome {
    pub scene_component_action: ModifierSceneComponentInitAction,
    /// `Some(world_field_5a0)` written to `*(u32*)(this + 0xb0)` when `+0x300 != 0`.
    pub snapshot_world_5a0_into_b0: Option<u32>,
    /// Clears `*(u64*)(this + 0x3c0) = 0` and registers looping `HighFrequencyTick`
    /// (`Rate = 0.1f32`, `bLoop = true`, `FirstDelay = -1.0f32`) when `+0x300 != 0`.
    pub high_frequency_timer_3c0: Option<ModifierLoopingTimerSetup>,
    /// Calls `ModifierTick(this, 0.0f32)` (`0x141c9d420` server / `0x141c8f4e0`
    /// client) iff `+0x301 != 0 && CheckTickConditions(this)`.
    pub run_initial_modifier_tick_zero_dt: bool,
}

/// Evaluates `UModifierStateComponent::InitComponent_Implementation`
/// (`vftable + 0x430`: `0x1416e88f0` server / `0x1416fa330` client).
pub const fn evaluate_modifier_init_component_impl(
    inputs: &ModifierInitComponentInputs,
) -> ModifierInitComponentOutcome {
    let scene_component_action = if !inputs.soft_scene_class_path_non_empty_1b0 {
        ModifierSceneComponentInitAction::SkippedEmptySoftPath
    } else if !inputs.resolved_valid_scene_component_class {
        ModifierSceneComponentInitAction::NullClassConstructCrash
    } else {
        ModifierSceneComponentInitAction::ConstructRegisterAndAttachKeepRelative {
            attachment_rules_packed: MODIFIER_SCENE_ATTACH_KEEP_RELATIVE_PACKED,
        }
    };

    if matches!(
        scene_component_action,
        ModifierSceneComponentInitAction::NullClassConstructCrash
    ) {
        return ModifierInitComponentOutcome {
            scene_component_action,
            snapshot_world_5a0_into_b0: None,
            high_frequency_timer_3c0: None,
            run_initial_modifier_tick_zero_dt: false,
        };
    }

    let (snapshot_world_5a0_into_b0, high_frequency_timer_3c0) =
        if inputs.enable_high_frequency_tick_300 {
            let timer_manager = if inputs.world_has_owning_game_instance {
                ModifierTimerClearTarget::GameInstanceTimerManager
            } else {
                ModifierTimerClearTarget::WorldTimerManager
            };
            (
                Some(inputs.world_field_5a0),
                Some(ModifierLoopingTimerSetup {
                    timer_manager,
                    rate_seconds: f32::from_bits(MODIFIER_HIGH_FREQUENCY_TICK_RATE_BITS),
                    loop_timer: true,
                    first_delay_seconds: MODIFIER_TIMER_DEFAULT_FIRST_DELAY,
                }),
            )
        } else {
            (None, None)
        };

    ModifierInitComponentOutcome {
        scene_component_action,
        snapshot_world_5a0_into_b0,
        high_frequency_timer_3c0,
        run_initial_modifier_tick_zero_dt: inputs.tick_immediately_301
            && inputs.check_tick_conditions_passed,
    }
}

/// Outcome of `UModifierStateComponent::ModifierApplied_Implementation`
/// (`vftable + 0x410`: `0x1416eae50` server `384` B / `0x1416fca50` client `306` B).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierAppliedOutcome {
    /// `OwnerPrivate` (`+0xa0`) is `nullptr`: returns `false` without mutating state.
    MissingOwnerFalse,
    /// `OwnerPrivate` is non-null: populates `+0xe0`, `+0xe8`, `+0xf0`
    /// (`GetInterfaceAddress(UModifiableInterface)`), `+0xf8`
    /// (`Execute_GetNextUID(OwnerPrivate)`), and `*(u8*)(this + 0x3b0) = 1`;
    /// populates `+0xd0` from `OwnerPrivate->GetInstigatorController()` iff
    /// `+0xd0 == nullptr`; calls `RefreshLifetime(this)`; appends `this + 0x320`
    /// gameplay tags to `OwnerPrivate` iff `*(i32*)(this + 0x328) != 0`; fires
    /// `Execute_OnModifiersUpdated(OwnerPrivate, this, false)` and
    /// `OnModifierUpdated.Broadcast(this, false)` (`+0x12c`); returns `true`.
    AppliedTrue {
        assigned_uid_f8: i32,
        set_flag_3b0: bool,
        populate_instigator_controller_d0_from_owner: bool,
        call_refresh_lifetime: bool,
        append_granted_tags_to_owner: bool,
        notify_owner_modifiable_interface_added: bool,
        broadcast_on_modifier_updated_added: bool,
    },
}

/// Evaluates `UModifierStateComponent::ModifierApplied_Implementation`
/// (`vftable + 0x410`: `0x1416eae50` server / `0x1416fca50` client).
pub const fn evaluate_modifier_applied_impl(
    owner_present: bool,
    next_uid_from_owner: i32,
    instigator_controller_d0_already_set: bool,
    granted_tags_count_328: i32,
) -> ModifierAppliedOutcome {
    if !owner_present {
        return ModifierAppliedOutcome::MissingOwnerFalse;
    }
    ModifierAppliedOutcome::AppliedTrue {
        assigned_uid_f8: next_uid_from_owner,
        set_flag_3b0: true,
        populate_instigator_controller_d0_from_owner: !instigator_controller_d0_already_set,
        call_refresh_lifetime: true,
        append_granted_tags_to_owner: granted_tags_count_328 != 0,
        notify_owner_modifiable_interface_added: true,
        broadcast_on_modifier_updated_added: true,
    }
}

/// Granted-tag view of one `UModifierStateComponent` on `OwnerPrivate` during
/// `UModifierStateComponent::ModifierRemoved_Implementation` (`vftable + 0x418`:
/// `0x1416eafd0` server / `0x1416fcb90` client, `899` B).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SiblingModifierGrantedTags<'a> {
    /// Whether this component pointer equals `this` (`CMP RCX, R13; JZ skip`).
    pub is_self: bool,
    /// `other_data.GrantedTags.GameplayTags` (`FModifierStateData + 0x1e0`),
    /// represented as 64-bit `FName` keys (`0` = `NAME_None`).
    pub gameplay_tags: &'a [u64],
    /// `other_data.GrantedTags.ParentTags` (`FModifierStateData + 0x1f0`),
    /// represented as 64-bit `FName` keys (`0` = `NAME_None`).
    pub parent_tags: &'a [u64],
}

/// Outcome of `UModifierStateComponent::ModifierRemoved_Implementation`
/// (`vftable + 0x418`: `0x1416eafd0` server / `0x1416fcb90` client, `899` B).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ModifierRemovedOutcome {
    /// `OwnerPrivate` (`+0xa0`) is `nullptr`: returns `false`.
    MissingOwnerFalse,
    /// `*(i32*)(this + 0x328) == 0` (`GrantedTags.GameplayTags.Num() == 0`):
    /// skips sibling enumeration and owner tag mutation, returning `true`.
    EmptyGrantedTagsTrue,
    /// Copies `this + 0x320` into `removal_set`, subtracts every valid tag
    /// (`tag != 0`) still covered by another modifier's `GrantedTags.GameplayTags`
    /// or `GrantedTags.ParentTags` (skipping siblings whose `GameplayTags` is
    /// empty and breaking early once `removal_set` becomes empty), then calls
    /// `GetGameplayTagContainer` + `RemoveTags(removal_set)` +
    /// `SetGameplayTagContainer` on `OwnerPrivate` and returns `true`.
    UpdatedOwnerTagsTrue {
        siblings_inspected: usize,
        remaining_tags_to_remove: Vec<u64>,
    },
}

/// Evaluates `UModifierStateComponent::ModifierRemoved_Implementation`
/// (`vftable + 0x418`: `0x1416eafd0` server / `0x1416fcb90` client).
pub fn evaluate_modifier_removed_impl(
    owner_present: bool,
    self_granted_gameplay_tags_320: &[u64],
    owner_modifiers: &[SiblingModifierGrantedTags<'_>],
) -> ModifierRemovedOutcome {
    if !owner_present {
        return ModifierRemovedOutcome::MissingOwnerFalse;
    }
    if self_granted_gameplay_tags_320.is_empty() {
        return ModifierRemovedOutcome::EmptyGrantedTagsTrue;
    }

    let mut removal_set = self_granted_gameplay_tags_320.to_vec();
    let mut siblings_inspected = 0usize;

    for comp in owner_modifiers {
        if removal_set.is_empty() {
            break;
        }
        if comp.is_self {
            continue;
        }
        siblings_inspected += 1;
        if comp.gameplay_tags.is_empty() {
            continue;
        }
        let mut idx = removal_set.len();
        while idx > 0 {
            idx -= 1;
            let tag = if idx < removal_set.len() {
                removal_set[idx]
            } else {
                0
            };
            // `0` represents `FName` with `ComparisonIndex == NAME_None && Number == 0`.
            if tag != 0 && (comp.gameplay_tags.contains(&tag) || comp.parent_tags.contains(&tag)) {
                removal_set.retain(|&existing| existing != tag);
            }
        }
    }

    ModifierRemovedOutcome::UpdatedOwnerTagsTrue {
        siblings_inspected,
        remaining_tags_to_remove: removal_set,
    }
}

/// Inputs for `UModifierStateComponent::OnStatContainerUpdated` (`0x1416efd80`
/// server `1,139` B / `0x1417012a0` client `979` B).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModifierOnStatContainerUpdatedInputs<'a> {
    pub old_lifetime_multiplier_10c: f32,
    pub new_lifetime_multiplier: f32,
    pub has_active_auto_destroy_timer_3a8: bool,
    pub world_has_owning_game_instance: bool,
    pub base_lifetime_108: f32,
    pub remaining_time_110: f32,
    /// Whether cached owner pointer `*(UObject**)(this + 0xe0)` is non-null and
    /// its `vtable[+0x620](UIcarusStatContainer::StaticClass())` returned a
    /// non-null `UIcarusStatContainer*`.
    pub owner_e0_has_stat_container: bool,
    /// Values returned by `UIcarusStatContainer::GetStat` for each entry in the
    /// `TArray<FStatsEnum>` at `this + 0x230` (`Num` at `this + 0x238`).
    pub effectiveness_stat_values: &'a [i32],
    /// Base effectiveness offset at `*(i32*)(this + 0x114)`.
    pub base_effectiveness_114: i32,
    /// Previous effectiveness cached at `*(i32*)(this + 0x128)`.
    pub old_effectiveness_128: i32,
    /// `UActorComponent::GetOwnerRole(this)` (`3` = `ROLE_Authority`).
    pub owner_role: i32,
}

/// Outcome of `UModifierStateComponent::OnStatContainerUpdated` (`0x1416efd80`
/// server / `0x1417012a0` client).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ModifierOnStatContainerUpdatedOutcome {
    /// `Some(new_lifetime_multiplier)` written to `+0x10c` when
    /// `UCOMISS old_mult, new_mult; JZ` is not taken (`!old.is_nan() &&
    /// !new.is_nan() && old != new`).
    pub updated_lifetime_multiplier_10c: Option<f32>,
    /// `Some(timer_manager)` cleared (and `+0x3a8` zeroed) when lifetime
    /// multiplier changed and `has_active_auto_destroy_timer_3a8` was true.
    pub cleared_auto_destroy_timer_3a8: Option<ModifierTimerClearTarget>,
    /// `Some(new_remaining_time)` passed to `SetRemainingTime(this, new_remaining)`
    /// when `base_lifetime_108 > 0.0`, `!(remaining_time_110 > base_lifetime_108 * old_mult)`,
    /// and `old_mult > 0.0`.
    pub rescaled_remaining_time_110: Option<f32>,
    /// Looping `AutoDestroy` timer started on `+0x3a8` when lifetime multiplier
    /// changed and `base_lifetime_108 > 0.0`.
    pub restarted_auto_destroy_timer_3a8: Option<ModifierLoopingTimerSetup>,
    /// `Some(new_effectiveness)` written to `+0x128` when the summed stat
    /// effectiveness differs from `old_effectiveness_128`.
    pub updated_effectiveness_128: Option<i32>,
    /// Broadcasts multicast delegate at `this + 0x130` iff effectiveness changed
    /// and `owner_role == 3`.
    pub broadcast_effectiveness_delegate_130: bool,
    /// Dispatches `this->vtable[+0x408](this)` (`UpdateStats`) iff effectiveness changed.
    pub dispatch_update_stats: bool,
}

/// Evaluates `UModifierStateComponent::OnStatContainerUpdated` (`0x1416efd80`
/// server / `0x1417012a0` client).
pub fn evaluate_modifier_on_stat_container_updated(
    inputs: &ModifierOnStatContainerUpdatedInputs<'_>,
) -> ModifierOnStatContainerUpdatedOutcome {
    let old_mult = inputs.old_lifetime_multiplier_10c;
    let new_mult = inputs.new_lifetime_multiplier;
    let timer_manager = if inputs.world_has_owning_game_instance {
        ModifierTimerClearTarget::GameInstanceTimerManager
    } else {
        ModifierTimerClearTarget::WorldTimerManager
    };

    // `UCOMISS XMM6, XMM0; JZ skip` sets ZF=1 when equal OR when either operand is NaN.
    let mult_changed = !old_mult.is_nan() && !new_mult.is_nan() && old_mult != new_mult;

    let mut updated_lifetime_multiplier_10c = None;
    let mut cleared_auto_destroy_timer_3a8 = None;
    let mut rescaled_remaining_time_110 = None;
    let mut restarted_auto_destroy_timer_3a8 = None;

    if mult_changed {
        updated_lifetime_multiplier_10c = Some(new_mult);
        if inputs.has_active_auto_destroy_timer_3a8 {
            cleared_auto_destroy_timer_3a8 = Some(timer_manager);
        }
        // `COMISS base_lifetime, 0.0; JBE skip` requires ordered `base_lifetime > 0.0`.
        if inputs.base_lifetime_108 > 0.0_f32 {
            let old_scaled_max = inputs.base_lifetime_108 * old_mult;
            let mut effective_remaining = inputs.remaining_time_110;
            // `COMISS remaining, old_scaled_max; JA skip_rescale` +
            // `COMISS old_mult, 0.0; JBE skip_rescale`.
            let not_ordered_greater = !matches!(
                inputs.remaining_time_110.partial_cmp(&old_scaled_max),
                Some(std::cmp::Ordering::Greater)
            );
            if not_ordered_greater && old_mult > 0.0_f32 {
                let ratio = inputs.remaining_time_110 / old_scaled_max;
                let rescaled = (inputs.base_lifetime_108 * ratio) * new_mult;
                rescaled_remaining_time_110 = Some(rescaled);
                effective_remaining = rescaled;
            }
            restarted_auto_destroy_timer_3a8 = Some(ModifierLoopingTimerSetup {
                timer_manager,
                rate_seconds: effective_remaining,
                loop_timer: true,
                first_delay_seconds: MODIFIER_TIMER_DEFAULT_FIRST_DELAY,
            });
        }
    }

    let mut stat_sum = 0i32;
    if inputs.owner_e0_has_stat_container {
        for &val in inputs.effectiveness_stat_values {
            stat_sum = stat_sum.wrapping_add(val);
        }
    }
    let new_effectiveness = inputs.base_effectiveness_114.wrapping_add(stat_sum);
    let effectiveness_changed = new_effectiveness != inputs.old_effectiveness_128;

    ModifierOnStatContainerUpdatedOutcome {
        updated_lifetime_multiplier_10c,
        cleared_auto_destroy_timer_3a8,
        rescaled_remaining_time_110,
        restarted_auto_destroy_timer_3a8,
        updated_effectiveness_128: effectiveness_changed.then_some(new_effectiveness),
        broadcast_effectiveness_delegate_130: effectiveness_changed
            && inputs.owner_role == MODIFIER_AUTHORITY_NET_ROLE,
        dispatch_update_stats: effectiveness_changed,
    }
}

/// Preconditions checked at the entry of
/// `UIcarusFunctionLibrary::FixModifierActivation` (`0x1414a0fc0` server /
/// `0x1414db100` client, `609` B).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixModifierActivationGuards {
    pub actor_present: bool,
    pub actor_implements_modifiable_interface: bool,
    pub target_row_handle_valid: bool,
    pub actor_has_world: bool,
    /// Target `FModifierStatesRowHandle::GetRowName()` 64-bit `FName` key.
    pub target_row_name: u64,
}

/// Candidate `UModifierStateComponent` visited on `Actor` during
/// `UIcarusFunctionLibrary::FixModifierActivation` (`0x1414a0fc0` server /
/// `0x1414db100` client).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FixModifierActivationCandidate {
    /// Caller-supplied identifier to track which component receives each action.
    pub component_token: usize,
    /// `comp->ModifierRowHandle.GetRowName()` 64-bit `FName` key (`+0xbc..+0xc4`).
    pub row_name: u64,
    /// `(*(u8*)(comp + 0x8b) & 0x02) != 0` (`UActorComponent::bIsBeingDestroyed`).
    pub b_is_being_destroyed: bool,
    /// `(*(u8*)(comp + 0x8b) & 0x01) != 0` (`UActorComponent::bAutoActivate`).
    pub b_auto_activate: bool,
    /// `*(i32*)(comp + 0xf8)` (`ModifierUID`).
    pub uid_f8: i32,
}

/// Virtual call dispatched on a sorted matching `UModifierStateComponent` at the
/// end of `UIcarusFunctionLibrary::FixModifierActivation`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FixModifierActivationAction {
    /// `comp->vtable[+0x278](comp, false)` (`Activate(bReset = false)`).
    ActivateResetFalse { component_token: usize },
    /// `comp->vtable[+0x280](comp)` (`Deactivate()`).
    Deactivate { component_token: usize },
}

/// Evaluates `UIcarusFunctionLibrary::FixModifierActivation` (`0x1414a0fc0`
/// server / `0x1414db100` client, `609` B).
///
/// Matching `!bIsBeingDestroyed` components are inserted via UE4's
/// `AlgoImpl::LowerBoundInternal` (`sorted[mid].uid_f8 < comp.uid_f8`), placing
/// later-visited equal-`uid_f8` components before earlier-visited ones. Walking
/// the sorted array activates (`Activate(false)`) only the first component with
/// `b_auto_activate == true` and deactivates (`Deactivate()`) every other match.
pub fn select_fix_modifier_activation_actions(
    guards: &FixModifierActivationGuards,
    candidates: &[FixModifierActivationCandidate],
) -> Vec<FixModifierActivationAction> {
    if !guards.actor_present
        || !guards.actor_implements_modifiable_interface
        || !guards.target_row_handle_valid
        || !guards.actor_has_world
    {
        return Vec::new();
    }

    let mut sorted: Vec<FixModifierActivationCandidate> = Vec::new();
    for &cand in candidates {
        if cand.row_name != guards.target_row_name || cand.b_is_being_destroyed {
            continue;
        }
        let insert_idx = sorted.partition_point(|existing| existing.uid_f8 < cand.uid_f8);
        sorted.insert(insert_idx, cand);
    }

    let mut activated_any = false;
    let mut actions = Vec::with_capacity(sorted.len());
    for cand in sorted {
        if !activated_any && cand.b_auto_activate {
            activated_any = true;
            actions.push(FixModifierActivationAction::ActivateResetFalse {
                component_token: cand.component_token,
            });
        } else {
            actions.push(FixModifierActivationAction::Deactivate {
                component_token: cand.component_token,
            });
        }
    }
    actions
}

/// Caller-supplied view of one already-enumerated valid, non-null component for
/// the local post-enumeration selection in `RemoveModifierState`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoveModifierStateCandidate {
    /// Raw byte at component `+0x8b`; mask `0x02` excludes this candidate.
    pub flags_8b: u8,
    /// Signed component UID at `+0xf8`.
    pub uid_f8: i32,
    /// Provided opaque result for a comparison invocation using the saved row
    /// argument and component `+0xb4`. No row equality or callee purity is implied.
    /// This value is consumed only if this candidate's input index is compared.
    pub opaque_comparison_result: bool,
}

/// Why a candidate was selected by the local `RemoveModifierState` model.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RemoveModifierStateSelection {
    /// First retained exact UID match; its comparison and all later comparisons
    /// are bypassed, even if earlier comparisons returned true.
    ExplicitUid { input_index: usize },
    /// Last comparison-true candidate in retained enumeration order, used only
    /// when no explicit UID match was selected.
    LastComparisonTrue { input_index: usize },
}

/// A modeled request only; no virtual call is executed or destruction inferred.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RemoveModifierStateVirtualCallRequest {
    /// Index in the original caller-supplied candidate slice.
    pub input_index: usize,
    /// Requested virtual byte offset, always `+0x3c0`. F-930 supplies only the
    /// existing primary-slot baseline; it does not qualify subclass effects.
    pub vftable_byte_offset: u16,
    /// Requested EDX argument, always zero.
    pub edx: u32,
}

/// Local selection results, without owner validation, removal or lifecycle effects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RemoveModifierStateSelectionOutcome {
    /// All retained original input indices, in input order. Filtering completes
    /// before selection, including inputs after an eventual explicit UID match.
    pub filtered_input_indices: Vec<usize>,
    /// Original input indices whose opaque comparison results were consumed,
    /// in invocation order. Filtered-out inputs are never compared.
    pub compared_input_indices: Vec<usize>,
    pub selection: Option<RemoveModifierStateSelection>,
    /// Exactly one request on selection; none when no candidate is selected.
    pub virtual_call_request: Option<RemoveModifierStateVirtualCallRequest>,
    /// Modeled caller flag after the requested call returns. This is true on
    /// selection regardless of subclass effects; no call is made by this model.
    pub local_success_after_request_returns: bool,
}

/// Models only the local post-enumeration `RemoveModifierState` selection.
///
/// First filters all candidates by raw `flags_8b & 0x02`, preserving input order.
/// Then the first exact signed UID match wins immediately unless the requested
/// UID is `-1`. Every other visited candidate consumes its provided opaque bool;
/// absent an explicit match, the last true result wins. UID mismatch permits
/// fallback, and `-1` suppresses only the UID path, even for component UID `-1`.
///
/// Inputs must already describe valid, non-null candidates. This pure model does
/// not enumerate components, interpret row equality or invalid pointers, resolve
/// owner virtual lookups, perform logging, or invoke comparisons or virtual calls.
/// Its request and success-after-return flag establish no destruction, subclass
/// behavior, owner pipeline, import authority or stock-client compatibility.
pub fn select_remove_modifier_state_local(
    requested_uid: i32,
    candidates: &[RemoveModifierStateCandidate],
) -> RemoveModifierStateSelectionOutcome {
    let mut filtered_input_indices = Vec::new();
    for (input_index, candidate) in candidates.iter().enumerate() {
        if candidate.flags_8b & 0x02 == 0 {
            filtered_input_indices.push(input_index);
        }
    }

    let mut compared_input_indices = Vec::new();
    let mut selection = None;
    for &input_index in &filtered_input_indices {
        let candidate = &candidates[input_index];
        if requested_uid != -1 && requested_uid == candidate.uid_f8 {
            selection = Some(RemoveModifierStateSelection::ExplicitUid { input_index });
            break;
        }
        compared_input_indices.push(input_index);
        if candidate.opaque_comparison_result {
            selection = Some(RemoveModifierStateSelection::LastComparisonTrue { input_index });
        }
    }

    let virtual_call_request = match selection {
        Some(RemoveModifierStateSelection::ExplicitUid { input_index })
        | Some(RemoveModifierStateSelection::LastComparisonTrue { input_index }) => {
            Some(RemoveModifierStateVirtualCallRequest {
                input_index,
                vftable_byte_offset: 0x3c0,
                edx: 0,
            })
        }
        None => None,
    };
    RemoveModifierStateSelectionOutcome {
        filtered_input_indices,
        compared_input_indices,
        selection,
        virtual_call_request,
        local_success_after_request_returns: virtual_call_request.is_some(),
    }
}

/// Authored identities for the two distinct scratch slots in the local comparator.
/// These labels specify call order, without assigning addresses or data meanings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierComparatorScratchSlot {
    /// Scratch slot supplied to the first call, with the right operand.
    FirstRight,
    /// A different scratch slot supplied to the second call, with the left operand.
    SecondLeft,
}

/// Caller-supplied operations for an optional physical caller-local comparator model.
///
/// `normalize` names an opaque call, not a claim of canonicalization or purity.
/// Both operations may mutate shared state, and result tokens may alias. No
/// equality, copying or ownership semantics are required of operands or tokens.
pub trait ModifierComparatorOperations {
    /// Opaque input representation; left and right may refer to the same operand.
    type Operand: ?Sized;
    /// Opaque returned handle, retained across subsequent callback invocations.
    type ResultToken;

    /// Requests the same opaque operation with an operand and distinct scratch label.
    fn normalize(
        &mut self,
        operand: &Self::Operand,
        scratch_slot: ModifierComparatorScratchSlot,
    ) -> Self::ResultToken;

    /// Reads one exact `u64` through a result token at the time of this callback.
    fn read_u64(&mut self, result_token: &Self::ResultToken) -> u64;
}

/// Models only the bounded physical caller-local comparator operation order.
///
/// Left and right denote initial Windows ABI RCX and RDX respectively in the
/// qualified server `25621760` / client `25793767` entries. Both calls precede
/// either word read: right with first scratch, left with second scratch, then
/// reads through the left result and the saved right result. Exact `u64`
/// equality models the returned AL boolean; token identity is never compared.
///
/// The caller adds no semantic writes beyond its local scratch/token storage;
/// callback effects remain opaque. This supplies no callee semantics, row-name
/// interpretation, purity, unique C++ owner, logical-function completeness,
/// runtime qualification or import authority. It does not integrate with
/// persistence, the live native server or `RemoveModifierStateCandidate`.
pub fn evaluate_modifier_comparator_local<O: ModifierComparatorOperations + ?Sized>(
    operations: &mut O,
    left: &O::Operand,
    right: &O::Operand,
) -> bool {
    let right_result = operations.normalize(right, ModifierComparatorScratchSlot::FirstRight);
    let left_result = operations.normalize(left, ModifierComparatorScratchSlot::SecondLeft);
    let left_word = operations.read_u64(&left_result);
    let right_word = operations.read_u64(&right_result);
    left_word == right_word
}
