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
    assert_eq!(stats.get("CriticalDamage_+%"), 325);
    assert_eq!(stats.get("IsFirearm_?"), 0);
    // A rifle (11) is a firearm.
    let stats = StatValues::new(
        &names,
        values(&[
            ("ToolType_Enum", 11),
            ("BaseCriticalDamage_+%", 300),
            ("BaseFirearmCriticalDamage_+%", 40),
        ]),
    );
    assert_eq!(stats.get("IsFirearm_?"), 1);
    assert_eq!(stats.get("CriticalDamage_+%"), 340);
    // Atmosphere 9 selects the arctic bonus; the sum clamps to 75.
    let stats = StatValues::new(
        &names,
        values(&[
            ("Atmosphere_Enum", 9),
            ("BaseColdResistance_%", 60),
            ("BaseArcticColdResistance_+%", 30),
        ]),
    );
    assert_eq!(stats.get("ColdResistance_%"), 75);
    // Thirst level 1 halves the cooldown rate; level 2 floors it at 1.
    let stats = StatValues::new(
        &names,
        values(&[
            ("BaseCooldownDegreePerMinute_+", 25),
            ("BaseThirstLevel_+", 1),
        ]),
    );
    assert_eq!(stats.get("CooldownDegreePerMinute_+"), 12);
    let stats = StatValues::new(
        &names,
        values(&[
            ("BaseCooldownDegreePerMinute_+", 25),
            ("BaseThirstLevel_+", 2),
        ]),
    );
    assert_eq!(stats.get("CooldownDegreePerMinute_+"), 1);
    // Insulation: clamp(150) = 100, * 0.01 * (4200 - 6600) = -2400.
    let stats = StatValues::new(
        &names,
        values(&[
            ("BaseArmorInsulation_%", 150),
            ("BaseInternalTemperatureSafeRegionMaximum_+", 4200),
            ("BaseMaximumInternalTemperature_+", 6600),
        ]),
    );
    assert_eq!(stats.get("ArmorInsulation_%"), 100);
    assert_eq!(stats.get("ColdInsulation_+"), -2400);
    // Night (1) adds the night water bonus.
    let stats = StatValues::new(
        &names,
        values(&[
            ("TimeOfDay_Enum", 1),
            ("BaseWaterConsumption_+%", 5),
            ("BaseNightWaterConsumption_+%", 20),
        ]),
    );
    assert_eq!(stats.get("WaterConsumption_+%"), 25);
    // A name the table lacks reads 0.
    assert_eq!(stats.get("NoSuchStat_+"), 0);
}
