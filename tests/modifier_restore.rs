use std::collections::BTreeMap;

use icarus_bevy::experimental::modifier_restore::{
    KNOWN_BEHAVIOURS, MODIFIER_CLASS_CHECKSUM, ModifierRestoreGap, ModifierRestoreTables,
    ModifierRow, remaining_tenths, restore_modifiers,
};
use icarus_bevy::experimental::prospect_info::SavedModifierState;

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

/// Authored rows (not release data): a native one, a Blueprint one with an
/// affector, one not saved and one with an unobserved class.
fn tables() -> ModifierRestoreTables {
    let rows = vec![
        ModifierRow {
            saves: true,
            ..ModifierRow::default()
        },
        ModifierRow {
            saves: true,
            behaviour: Some(KNOWN_BEHAVIOURS[2].to_owned()),
            effectiveness_affectors: vec!["Affector".to_owned()],
        },
        ModifierRow {
            saves: false,
            ..ModifierRow::default()
        },
        ModifierRow {
            saves: true,
            behaviour: Some("/Game/Other.Other_C".to_owned()),
            ..ModifierRow::default()
        },
    ];
    ModifierRestoreTables {
        rows,
        row_index: BTreeMap::from([
            ("plain".to_owned(), 0),
            ("drink".to_owned(), 1),
            ("unsaved".to_owned(), 2),
            ("other".to_owned(), 3),
        ]),
    }
}

fn saved(row: &str, time_remaining: f32, effectiveness: i32) -> SavedModifierState {
    SavedModifierState {
        row_name: units(row),
        time_remaining,
        initial_lifetime: 300.0,
        duration_buff: 1.0,
        effectiveness,
    }
}

#[test]
fn remaining_time_replicates_in_whole_tenths() {
    assert_eq!(remaining_tenths(295.49), 2955);
    assert_eq!(remaining_tenths(0.05), 1);
    assert_eq!(remaining_tenths(-1.0), 0);
    assert_eq!(remaining_tenths(f32::NAN), 0);
    // 65536 tenths or more leaves a new component's field at 0.
    assert_eq!(remaining_tenths(6553.6), 0);
    assert_eq!(remaining_tenths(6553.5), 65_535);
}

#[test]
fn fractional_tenths_round_up_across_the_wire_range() {
    // Quarter-tenth samples stay well clear of f32 rounding boundaries.
    // The last sample exceeds the wire range and leaves the fresh field at 0.
    for whole in 0..=u16::MAX {
        let seconds = (f32::from(whole) + 0.25) / 10.0;
        assert_eq!(remaining_tenths(seconds), whole.wrapping_add(1));
    }
}

#[test]
fn quantization_preserves_float_boundaries_instead_of_using_real_number_ceil() {
    assert_eq!(remaining_tenths(0.1_f32.next_down()), 1);
    assert_eq!(remaining_tenths(0.1), 1);
    assert_eq!(remaining_tenths(0.1_f32.next_up()), 2);
    // Adding this scaled subnormal to -0.5 loses it in the pinned arithmetic.
    assert_eq!(remaining_tenths(f32::from_bits(1)), 0);
    assert_eq!(remaining_tenths(f32::INFINITY), 0);
    assert_eq!(remaining_tenths(f32::NEG_INFINITY), 0);
    assert_eq!(remaining_tenths(6553.5_f32.next_up()), 0);
}

#[test]
fn restored_modifiers_take_sequential_uids_in_saved_order() {
    let saved = [
        saved("Plain", 100.0, -1),
        saved("unsaved", 100.0, -1),
        saved("Missing", 100.0, -1),
        saved("Drink", -1.0, -1),
        saved("DRINK", 50.0, 150),
    ];
    let (restored, gaps) = restore_modifiers(&saved, &tables(), 1, 6, |stat| {
        assert_eq!(stat, "Affector");
        7
    });
    assert!(gaps.is_empty());
    assert_eq!(
        restored
            .iter()
            .map(|modifier| (modifier.row, modifier.uid, modifier.subobject))
            .collect::<Vec<_>>(),
        [(0, 1, 6), (1, 2, 7)]
    );
    // `-1` reads as 100, stored less 100; the affector stat adds.
    assert_eq!(restored[0].final_effectiveness, 0);
    assert_eq!(restored[1].final_effectiveness, 57);
    assert_eq!(restored[0].lifetime, 300.0);
    assert_eq!(restored[0].remaining_tenths, 1000);
    assert_eq!(restored[0].class[0].path, "ModifierStateComponent");
    assert_eq!(restored[0].class[1].path, "/Script/Icarus");
    assert_eq!(
        restored[1].class[0].path, "BP_Modifier_Drink_C",
        "a Blueprint class with its package"
    );
    assert_eq!(
        restored[1].class[1].path,
        "/Game/BP/Behaviours/Modifiers/BP_Modifier_Drink"
    );
    assert!(
        restored
            .iter()
            .all(|modifier| modifier.class[0].checksum == MODIFIER_CLASS_CHECKSUM)
    );
}

#[test]
fn the_duration_buff_divides_the_remaining_time() {
    let modifier = SavedModifierState {
        duration_buff: 2.0,
        ..saved("Plain", 100.0, -1)
    };
    let (restored, _) = restore_modifiers(&[modifier], &tables(), 1, 0, |_| 0);
    assert_eq!(restored[0].remaining_tenths, 500);

    let fractional = SavedModifierState {
        duration_buff: 2.0,
        ..saved("Plain", 0.3, -1)
    };
    let (restored, gaps) = restore_modifiers(&[fractional], &tables(), 1, 0, |_| 0);
    assert!(gaps.is_empty());
    assert_eq!(restored[0].remaining_tenths, 2);
}

#[test]
fn repeated_rows_and_unobserved_classes_are_left_out_as_gaps() {
    let saved = [
        saved("Plain", 10.0, -1),
        saved("Plain", 20.0, -1),
        saved("Other", 10.0, -1),
    ];
    let (restored, gaps) = restore_modifiers(&saved, &tables(), 1, 0, |_| 0);
    assert_eq!(restored.len(), 1);
    assert_eq!(
        gaps,
        [
            ModifierRestoreGap::RepeatedRow,
            ModifierRestoreGap::UnknownBehaviour
        ]
    );
}
