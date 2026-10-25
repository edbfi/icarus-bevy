//! Experimental `UModifierStateComponent` lifecycle and stat-container dispatch rules (F-930).
//!
//! Each function is a pure rule over caller-supplied component and owner state,
//! statically recovered from server `25621760` and client `25793767`
//! (`UModifierStateComponent` primary vftable `0x144a263a8` server / `0x144b94c08`
//! client, `Deactivate`, `Activate`, `UpdateStats`, `BeginPlay`, `EndPlay`,
//! `ModifierApplied` and `ModifierRemoved`). Blueprint `ProcessEvent` overrides,
//! per-stat scaling inside `0x1416b6710` / `0x1416ce2f0`, `InitComponent`
//! internals, packaged data-table row meanings (F-660) and live original-process
//! execution remain outside this module.

/// `EStatSources` discriminant (`3`) passed by `UModifierStateComponent::Deactivate`
/// and `UModifierStateComponent::UpdateStats` to `UIcarusStatContainer::RemoveStats_BP`
/// and `UIcarusStatContainer::AddStats` alongside `*(i32*)(this + 0xf8)` (F-930).
pub const MODIFIER_STAT_SOURCE_KIND: u8 = 3;

/// `ENetRole::ROLE_Authority` (`3`) required by `UModifierStateComponent::BeginPlay`
/// and the authority teardown block of `UModifierStateComponent::EndPlay` (F-930).
pub const MODIFIER_AUTHORITY_NET_ROLE: i32 = 3;

/// Byte value at `this + 0x319` (`3`) that triggers
/// `UIcarusFunctionLibrary::FixModifierActivation(OwnerPrivate, &ModifierRowHandle)`
/// inside the authority `EndPlay(0)` block (F-930).
pub const MODIFIER_FIX_ACTIVATION_STATE: u8 = 3;

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
    /// `+0x408`: `UModifierStateComponent::UpdateStats` (`0x1416fea90` server / `0x14170f320` client).
    ModifierUpdateStats,
    /// `+0x410`: `UModifierStateComponent::ModifierApplied_Implementation`
    /// (`0x1416eae50` server / `0x1416fca50` client).
    ModifierAppliedImplementation,
    /// `+0x418`: `UModifierStateComponent::ModifierRemoved_Implementation`
    /// (`0x1416eafd0` server / `0x1416fcb90` client).
    ModifierRemovedImplementation,
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
        0x408 => Some(ModifierPrimaryVftableSlot::ModifierUpdateStats),
        0x410 => Some(ModifierPrimaryVftableSlot::ModifierAppliedImplementation),
        0x418 => Some(ModifierPrimaryVftableSlot::ModifierRemovedImplementation),
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

/// Which `FTimerManager` receives `InternalClearAllTimers(this)` (`0x143977050`
/// server / `0x143c40dd0` client) inside `UModifierStateComponent::EndPlay(0)`
/// when `GetOwnerRole(this) == 3` (F-930).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierTimerClearTarget {
    /// Non-authority role, or both `WorldPrivate` (`this + 0xa8`) and
    /// `GetWorld_Uncached(this)` returned `nullptr`.
    None,
    /// `World->OwningGameInstance` (`World + 0x180`) is non-null: clears timers
    /// on `*(FTimerManager**)(OwningGameInstance + 0xd0)`.
    GameInstanceTimerManager,
    /// `World->OwningGameInstance` (`World + 0x180`) is `nullptr`: clears timers
    /// on `*(FTimerManager**)(World + 0x368)`.
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
