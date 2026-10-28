//! A fresh player's replicated pawn stats from authored data tables
//! (F-838, F-839).
#[path = "pak_writer/mod.rs"]
mod pak_writer;

use std::collections::BTreeMap;

use icarus_bevy::data_pak::DataTables;
use icarus_bevy::experimental::rep_layout::StatPairRepState;
use icarus_bevy::pawn_stats::{StatValues, fresh_player_stats};
use tempfile::TempDir;

fn tables(extra: &[(&str, Vec<u8>)]) -> (TempDir, DataTables) {
    let root = TempDir::new().unwrap();
    let path = root.path().join("data.pak");
    std::fs::write(&path, pak_writer::official_pak(extra)).unwrap();
    let tables = DataTables::open(&path).unwrap();
    (root, tables)
}

fn starting(grants: &[(&str, i64)]) -> Vec<u8> {
    let granted: serde_json::Map<String, serde_json::Value> = grants
        .iter()
        .map(|(name, value)| (format!("(Value=\"{name}\")"), (*value).into()))
        .collect();
    serde_json::to_vec(&serde_json::json!({"Rows": [
        {"Name": "Base_Stats", "StatsGranted": granted},
    ]}))
    .unwrap()
}

fn pair(stat: u32, value: i32) -> StatPairRepState {
    StatPairRepState { stat, value }
}

/// Replicated rows with a nonzero value, in row order: maxima scaled from
/// their bases, the movement speed and the sprint speed derived from it
/// (truncating), a granted flag; unreplicated and zero rows stay out.
#[test]
fn replicated_stats_follow_the_formulas_in_row_order() {
    let (_root, mut tables) = tables(&[(
        "Stats/D_CharacterStartingStats.json",
        starting(&[
            ("BaseMaximumHealth_+", 300),
            ("BaseMaximumHealth_+%", 10),
            ("BaseWeightCapacity_+", 100),
            ("BaseMovementSpeed_+", 355),
            ("SprintMovementSpeedCoefficient_%", 201),
            ("CanSeeCompass_?", 1),
        ]),
    )]);
    let stats = fresh_player_stats(&mut tables, "D_CharacterStartingStats", "Base_Stats").unwrap();
    assert_eq!(
        stats.items,
        [
            // (10 + 100) * 300 / 100.
            pair(0, 330),
            // MaximumStamina_+ has no base: its floor of 1.
            pair(3, 1),
            pair(10, 355),
            // 201 * 355 / 100 = 713 (713.55 truncated), * (0 + 100) / 100.
            pair(12, 713),
            pair(14, 1),
        ]
    );
    assert_eq!(stats.replication_key, 5);
}

#[test]
fn malformed_or_unknown_grants_fail() {
    for (bytes, expected) in [
        (starting(&[("NoSuchStat_+", 1)]), "lacks a granted stat"),
        (
            serde_json::to_vec(&serde_json::json!({"Rows": [
                {"Name": "Base_Stats", "StatsGranted": {"BaseMaximumHealth_+": 1}},
            ]}))
            .unwrap(),
            "not a stat row handle",
        ),
        (
            starting(&[("BaseMaximumHealth_+", 1i64 << 40)]),
            "not an int",
        ),
    ] {
        let (_root, mut tables) = tables(&[("Stats/D_CharacterStartingStats.json", bytes)]);
        let error = fresh_player_stats(&mut tables, "D_CharacterStartingStats", "Base_Stats")
            .expect_err("fails");
        assert!(
            format!("{error:#}").contains(expected),
            "{expected}: {error:#}"
        );
    }
}

/// The modelled formulas against hand-computed values: selections by tool
/// type and atmosphere, flags, clamps and the single-precision temperature
/// terms.
#[test]
fn virtual_formulas_select_clamp_and_scale() {
    let names: Vec<String> = [
        "ToolType_Enum",
        "Atmosphere_Enum",
        "TimeOfDay_Enum",
        "BaseCriticalDamage_+%",
        "BaseKnifeCriticalDamage_+%",
        "BaseFirearmCriticalDamage_+%",
        "CriticalDamage_+%",
        "IsFirearm_?",
        "BaseColdResistance_%",
        "BaseArcticColdResistance_+%",
        "ColdResistance_%",
        "BaseCooldownDegreePerMinute_+",
        "ThirstLevel_+",
        "BaseThirstLevel_+",
        "CooldownDegreePerMinute_+",
        "BaseArmorInsulation_%",
        "ArmorInsulation_%",
        "BaseInternalTemperatureSafeRegionMaximum_+",
        "BaseMaximumInternalTemperature_+",
        "ColdInsulation_+",
        "BaseNightWaterConsumption_+%",
        "BaseWaterConsumption_+%",
        "WaterConsumption_+%",
    ]
    .iter()
    .map(|name| name.to_string())
    .collect();
    let at = |name: &str| names.iter().position(|row| row == name).unwrap();
    let values = |grants: &[(&str, i32)]| {
        let sources: BTreeMap<usize, i32> = grants
            .iter()
            .map(|(name, value)| (at(name), *value))
            .collect();
        sources
    };
    // A knife (3): the knife bonus, no firearm bonus.
    let sources = values(&[
        ("ToolType_Enum", 3),
        ("BaseCriticalDamage_+%", 300),
        ("BaseKnifeCriticalDamage_+%", 25),
        ("BaseFirearmCriticalDamage_+%", 40),
    ]);
    let stats = StatValues::new(&names, sources);
    assert_eq!(stats.value("CriticalDamage_+%"), 325);
    assert_eq!(stats.value("IsFirearm_?"), 0);
    // A rifle (11) is a firearm.
    let stats = StatValues::new(
        &names,
        values(&[
            ("ToolType_Enum", 11),
            ("BaseCriticalDamage_+%", 300),
            ("BaseFirearmCriticalDamage_+%", 40),
        ]),
    );
    assert_eq!(stats.value("IsFirearm_?"), 1);
    assert_eq!(stats.value("CriticalDamage_+%"), 340);
    // Atmosphere 9 selects the arctic bonus; the sum clamps to 75.
    let stats = StatValues::new(
        &names,
        values(&[
            ("Atmosphere_Enum", 9),
            ("BaseColdResistance_%", 60),
            ("BaseArcticColdResistance_+%", 30),
        ]),
    );
    assert_eq!(stats.value("ColdResistance_%"), 75);
    // Thirst level 1 halves the cooldown rate; level 2 floors it at 1.
    let stats = StatValues::new(
        &names,
        values(&[
            ("BaseCooldownDegreePerMinute_+", 25),
            ("BaseThirstLevel_+", 1),
        ]),
    );
    assert_eq!(stats.value("CooldownDegreePerMinute_+"), 12);
    let stats = StatValues::new(
        &names,
        values(&[
            ("BaseCooldownDegreePerMinute_+", 25),
            ("BaseThirstLevel_+", 2),
        ]),
    );
    assert_eq!(stats.value("CooldownDegreePerMinute_+"), 1);
    // Insulation: clamp(150) = 100, * 0.01 * (4200 - 6600) = -2400.
    let stats = StatValues::new(
        &names,
        values(&[
            ("BaseArmorInsulation_%", 150),
            ("BaseInternalTemperatureSafeRegionMaximum_+", 4200),
            ("BaseMaximumInternalTemperature_+", 6600),
        ]),
    );
    assert_eq!(stats.value("ArmorInsulation_%"), 100);
    assert_eq!(stats.value("ColdInsulation_+"), -2400);
    // Night (1) adds the night water bonus.
    let stats = StatValues::new(
        &names,
        values(&[
            ("TimeOfDay_Enum", 1),
            ("BaseWaterConsumption_+%", 5),
            ("BaseNightWaterConsumption_+%", 20),
        ]),
    );
    assert_eq!(stats.value("WaterConsumption_+%"), 25);
    // A name the table lacks reads 0.
    assert_eq!(stats.value("NoSuchStat_+"), 0);
}

/// Extreme grants wrap as the original's 32-bit arithmetic does instead of
/// overflowing.
#[test]
fn extreme_grants_wrap_instead_of_overflowing() {
    let names: Vec<String> = [
        "BaseMaximumHealth_+",
        "BaseMaximumHealth_+%",
        "MaximumHealth_+",
        "BaseMovementSpeed_+",
        "MovementSpeed_+",
        "SprintMovementSpeedCoefficient_%",
        "SprintSpeed_+",
    ]
    .iter()
    .map(|name| name.to_string())
    .collect();
    let sources: BTreeMap<usize, i32> =
        [(0, i32::MAX), (1, i32::MAX), (3, i32::MAX), (5, i32::MAX)]
            .into_iter()
            .collect();
    let stats = StatValues::new(&names, sources);
    let health = (std::num::Wrapping(i32::MAX) + std::num::Wrapping(100))
        * std::num::Wrapping(i32::MAX)
        / std::num::Wrapping(100);
    assert_eq!(stats.value("MaximumHealth_+"), health.0.max(1));
    // 100 * i32::MAX wraps to -100; / 100 = -1, floored at 0; so the
    // sprint speed over it is (i32::MAX * 0 / 100 + 0) * 100 / 100 = 0.
    assert_eq!(stats.value("MovementSpeed_+"), 0);
    assert_eq!(stats.value("SprintSpeed_+"), 0);
}

/// The formulas a returning pawn's restored stats add (F-984), on authored
/// sources.
#[test]
fn restored_stat_formulas_combine_their_bases() {
    let names: Vec<String> = [
        "ToolType_Enum",
        "Atmosphere_Enum",
        "TimeOfDay_Enum",
        "IsMounted_?",
        "IsOverencumbered_?",
        "BaseBackpackSlots_+",
        "BackpackSlots_+",
        "BaseFallDamageResistance_%",
        "FallDamageResistance_%",
        "BasePhysicalDamageResistance_%",
        "BasePhysicalDamageResistance_+%",
        "BasePhysicalDamageResistanceWhileFishing_%",
        "PhysicalDamageResistance_%",
        "BaseMeleeDamageResistance_%",
        "BaseMeleeDamageResistance_+%",
        "BaseCarryingSpearMeleeDamageResistance_+%",
        "MeleeDamageResistance_+%",
        "MeleeDamageResistance_%",
        "BaseProjectileDamageResistance_%",
        "BaseProjectileDamageResistance_+%",
        "ProjectileDamageResistance_%",
        "BaseStaminaActionCost_+%",
        "BaseOverEncumberedStaminaActionCost_+%",
        "StaminaActionCost_+%",
        "BaseJumpingStaminaActionCost_+%",
        "JumpingStaminaActionCost_+%",
        "BaseToolStaminaActionCost_+%",
        "BaseKnifeStaminaActionCost_+%",
        "ToolStaminaActionCost_+%",
        "BaseExposureResistance_+%",
        "BaseCaveExposureResistance_+%",
        "BaseMountedExposureResistance_+%",
        "MountedExposureResistance_+%",
        "ExposureResistance_+%",
        "BaseItemWear_+%",
        "BaseItemWearAtNight_+%",
        "BaseKnifeItemWear_+%",
        "BaseLureItemWear_+%",
        "BaseFirearmItemWear_+%",
        "IsFirearm_?",
        "ItemWear_+%",
        "LureItemWear_+%",
        "BaseSprainModifierDuration_+%",
        "BaseModifierDuration_+%",
        "SprainModifierDuration_+%",
    ]
    .iter()
    .map(|name| name.to_string())
    .collect();
    let at = |name: &str| names.iter().position(|row| row == name).unwrap();
    let stats = |grants: &[(&str, i32)]| {
        StatValues::new(
            &names,
            grants
                .iter()
                .map(|(name, value)| (at(name), *value))
                .collect(),
        )
    };
    // A knife (3) in a cave (0) at night (1), mounted and overencumbered.
    let knife = stats(&[
        ("ToolType_Enum", 3),
        ("Atmosphere_Enum", 0),
        ("TimeOfDay_Enum", 1),
        ("IsMounted_?", 1),
        ("IsOverencumbered_?", 1),
        ("BaseBackpackSlots_+", 6),
        ("BaseFallDamageResistance_%", 150),
        ("BasePhysicalDamageResistance_%", 45),
        ("BasePhysicalDamageResistance_+%", 10),
        ("BasePhysicalDamageResistanceWhileFishing_%", 30),
        ("BaseMeleeDamageResistance_%", 5),
        ("BaseMeleeDamageResistance_+%", 20),
        ("BaseCarryingSpearMeleeDamageResistance_+%", 50),
        ("BaseProjectileDamageResistance_%", 7),
        ("BaseProjectileDamageResistance_+%", 50),
        ("BaseStaminaActionCost_+%", -15),
        ("BaseOverEncumberedStaminaActionCost_+%", 40),
        ("BaseJumpingStaminaActionCost_+%", -60),
        ("BaseToolStaminaActionCost_+%", 10),
        ("BaseKnifeStaminaActionCost_+%", -5),
        ("BaseExposureResistance_+%", 25),
        ("BaseCaveExposureResistance_+%", 3),
        ("BaseMountedExposureResistance_+%", 7),
        ("BaseItemWear_+%", -20),
        ("BaseItemWearAtNight_+%", -4),
        ("BaseKnifeItemWear_+%", -10),
        ("BaseLureItemWear_+%", -30),
        ("BaseFirearmItemWear_+%", -50),
        ("BaseSprainModifierDuration_+%", -50),
        ("BaseModifierDuration_+%", 10),
    ]);
    assert_eq!(knife.value("BackpackSlots_+"), 6);
    // Resistances cap at 99.
    assert_eq!(knife.value("FallDamageResistance_%"), 99);
    // (10 + 100) * 45 / 100; fishing's bonus only with a lure (13).
    assert_eq!(knife.value("PhysicalDamageResistance_%"), 49);
    // The spear's bonus only with a spear (4): (5 + 49) * (20 + 100) / 100.
    assert_eq!(knife.value("MeleeDamageResistance_+%"), 20);
    assert_eq!(knife.value("MeleeDamageResistance_%"), 64);
    // (50 + 100) * 7 / 100 + 49.
    assert_eq!(knife.value("ProjectileDamageResistance_%"), 59);
    // Overencumbered: -15 + 40; jumping and the knife add their own.
    assert_eq!(knife.value("StaminaActionCost_+%"), 25);
    assert_eq!(knife.value("JumpingStaminaActionCost_+%"), -35);
    assert_eq!(knife.value("ToolStaminaActionCost_+%"), 30);
    // Base, the cave's and, mounted, the mounted pass-through.
    assert_eq!(knife.value("MountedExposureResistance_+%"), 7);
    assert_eq!(knife.value("ExposureResistance_+%"), 35);
    // Base, night's and the knife's; no firearm or lure term.
    assert_eq!(knife.value("ItemWear_+%"), -34);
    assert_eq!(knife.value("LureItemWear_+%"), -24);
    assert_eq!(knife.value("SprainModifierDuration_+%"), -40);
    // A lure (13) by day (3), on foot and unburdened: fishing's resistance,
    // the lure's wear twice for item wear (its tool entry), once for lures.
    let lure = stats(&[
        ("ToolType_Enum", 13),
        ("TimeOfDay_Enum", 3),
        ("BasePhysicalDamageResistance_%", 45),
        ("BasePhysicalDamageResistanceWhileFishing_%", 30),
        ("BaseStaminaActionCost_+%", -15),
        ("BaseOverEncumberedStaminaActionCost_+%", 40),
        ("BaseMountedExposureResistance_+%", 7),
        ("BaseItemWear_+%", -20),
        ("BaseItemWearAtNight_+%", -4),
        ("BaseLureItemWear_+%", -30),
    ]);
    assert_eq!(lure.value("PhysicalDamageResistance_%"), 75);
    assert_eq!(lure.value("StaminaActionCost_+%"), -15);
    assert_eq!(lure.value("ExposureResistance_+%"), 0);
    assert_eq!(lure.value("ItemWear_+%"), -50);
    assert_eq!(lure.value("LureItemWear_+%"), -50);
    // A firearm (10) adds its own wear.
    let rifle = stats(&[("ToolType_Enum", 10), ("BaseFirearmItemWear_+%", -50)]);
    assert_eq!(rifle.value("IsFirearm_?"), 1);
    assert_eq!(rifle.value("ItemWear_+%"), -50);
}
