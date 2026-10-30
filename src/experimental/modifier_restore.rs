//! A returning pawn's saved modifiers restored as the original's base
//! actor-state restore does (F-965, F-980).
//!
//! For each saved `ModifierStateSaveData`, in saved order, the restore needs
//! a non-negative `TimeRemaining`, a valid `D_ModifierStates` row and the
//! row's `SaveToDatabase`; `AddModifierState` then creates the row's
//! `Behaviour` component (else a `UModifierStateComponent`) with the saved
//! effectiveness (`-1` reads as 100) stored less 100, and
//! `SetReloadedModifierInitialLifeTime` sets the lifetime to the saved
//! initial lifetime and the remaining time to the saved remaining time over
//! its duration buff. The client receives each as a new class-named
//! sub-object of the pawn carrying its row, owner, UID, lifetime, final
//! effectiveness and remaining time in tenths of a second.
//!
//! The final effectiveness adds the pawn's values of the row's
//! `ModifierEffectivenessAffectors` stats (`GetCurrentModifierEffectiveness`),
//! which the caller supplies. A row restored twice (its merge rule) and
//! component classes whose network checksum is not known are not modelled:
//! such a modifier is left out and counted.
use std::collections::BTreeMap;

use super::prospect_info::SavedModifierState;
use super::session_open::PathSegment;

/// The class network checksum of `UModifierStateComponent` and of every
/// modifier Blueprint observed so far, none of which adds replicated
/// properties (M-1110).
pub const MODIFIER_CLASS_CHECKSUM: u32 = 3_203_773_154;

/// Modifier `Behaviour` classes whose exports carried
/// [`MODIFIER_CLASS_CHECKSUM`] in the original's capture (M-1110).
pub const KNOWN_BEHAVIOURS: [&str; 6] = [
    "/Game/BP/Behaviours/Modifiers/BP_Modifier_Base.BP_Modifier_Base_C",
    "/Game/BP/Behaviours/Modifiers/BP_Modifier_Aura_Base.BP_Modifier_Aura_Base_C",
    "/Game/BP/Behaviours/Modifiers/BP_Modifier_Drink.BP_Modifier_Drink_C",
    "/Game/BP/Behaviours/Modifiers/BP_Modifier_Swimming.BP_Modifier_Swimming_C",
    "/Game/BP/Behaviours/Modifiers/BP_Modifier_Tamed.BP_Modifier_Tamed_C",
    "/Game/BP/Behaviours/Modifiers/BP_Modifier_Wet.BP_Modifier_Wet_C",
];

/// What restore needs of one `D_ModifierStates` row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModifierRow {
    /// `SaveToDatabase` (default true).
    pub saves: bool,
    /// `Behaviour`'s class path, when the row names one.
    pub behaviour: Option<String>,
    /// `ModifierEffectivenessAffectors`' stat names.
    pub effectiveness_affectors: Vec<String>,
}

/// `D_ModifierStates`, precomputed.
#[derive(Clone, Debug, Default)]
pub struct ModifierRestoreTables {
    /// By row index.
    pub rows: Vec<ModifierRow>,
    /// Lower-case row name to row index.
    pub row_index: BTreeMap<String, u16>,
}

/// One restored modifier as the client receives it.
#[derive(Clone, Debug, PartialEq)]
pub struct RestoredModifier {
    /// `DataRowHandleNew`: the `D_ModifierStates` row.
    pub row: u16,
    /// `ModifierUID`.
    pub uid: i32,
    /// `ModifierLifeTime`.
    pub lifetime: f32,
    /// `CachedFinalEffectiveness`: the stored effectiveness (saved less 100)
    /// plus the affector stats.
    pub final_effectiveness: i32,
    /// Raw restored binary32 seconds, independent of wire quantization.
    /// The computed restore input, not callback-adjusted runtime state.
    pub remaining_seconds: f32,
    /// `ReplicatedRemainingTime`: whole tenths of a second.
    pub remaining_tenths: u16,
    /// The component class with its package.
    pub class: Vec<PathSegment>,
    /// Its index among the pawn's runtime sub-objects.
    pub subobject: u32,
}

/// Why a saved modifier is left out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ModifierRestoreGap {
    RepeatedRow,
    UnknownBehaviour,
}

/// The component class chain for a `Behaviour` path, or the native class.
fn class_chain(behaviour: Option<&str>) -> Option<Vec<PathSegment>> {
    match behaviour {
        None => Some(vec![
            PathSegment::new("ModifierStateComponent", MODIFIER_CLASS_CHECKSUM, false),
            PathSegment::new("/Script/Icarus", 0, false),
        ]),
        Some(path) => {
            if !KNOWN_BEHAVIOURS.contains(&path) {
                return None;
            }
            let (package, class) = path.split_once('.')?;
            Some(vec![
                PathSegment::new(class, MODIFIER_CLASS_CHECKSUM, false),
                PathSegment::new(package, 0, false),
            ])
        }
    }
}

/// `SetRemainingTime`'s value for a new component, under round-to-nearest,
/// ties-to-even. The pinned setter doubles the scaled float, subtracts it
/// from -0.5, rounds to an integer, then negates its arithmetic half.
/// This rounds positive fractional tenths upward except where f32 precision
/// absorbs the fraction. Out-of-range values leave the new field at 0.
pub fn remaining_tenths(remaining: f32) -> u16 {
    super::modifier_timing::quantize_remaining_time(remaining).unwrap_or(0)
}

/// Restore `saved` onto a pawn whose modifier UIDs start at `first_uid`,
/// whose next runtime sub-object index is `first_subobject` and whose stat
/// `name` is `stat(name)`: the restored modifiers in order, and the gaps
/// that left modifiers out.
pub fn restore_modifiers(
    saved: &[SavedModifierState],
    tables: &ModifierRestoreTables,
    first_uid: i32,
    first_subobject: u32,
    stat: impl Fn(&str) -> i32,
) -> (Vec<RestoredModifier>, Vec<ModifierRestoreGap>) {
    let mut restored: Vec<RestoredModifier> = Vec::new();
    let mut gaps = Vec::new();
    let mut uid = first_uid;
    for modifier in saved {
        if modifier.time_remaining.is_nan() || modifier.time_remaining < 0.0 {
            continue;
        }
        let name = String::from_utf16_lossy(&modifier.row_name).to_ascii_lowercase();
        let Some(row) = tables.row_index.get(&name).copied() else {
            continue;
        };
        let Some(info) = tables.rows.get(usize::from(row)) else {
            continue;
        };
        if !info.saves {
            continue;
        }
        if restored.iter().any(|done| done.row == row) {
            gaps.push(ModifierRestoreGap::RepeatedRow);
            continue;
        }
        let Some(class) = class_chain(info.behaviour.as_deref()) else {
            gaps.push(ModifierRestoreGap::UnknownBehaviour);
            continue;
        };
        let effectiveness = if modifier.effectiveness == -1 {
            100
        } else {
            modifier.effectiveness
        };
        let remaining = if modifier.duration_buff > 0.0 {
            modifier.time_remaining / modifier.duration_buff
        } else {
            modifier.time_remaining
        };
        restored.push(RestoredModifier {
            row,
            uid,
            lifetime: modifier.initial_lifetime,
            final_effectiveness: info
                .effectiveness_affectors
                .iter()
                .fold(effectiveness.wrapping_sub(100), |total, name| {
                    total.wrapping_add(stat(name))
                }),
            remaining_seconds: remaining,
            remaining_tenths: remaining_tenths(remaining),
            class,
            subobject: first_subobject.wrapping_add(u32::try_from(restored.len()).unwrap_or(0)),
        });
        uid = uid.wrapping_add(1);
    }
    (restored, gaps)
}
