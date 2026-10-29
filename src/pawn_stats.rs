//! A fresh player's replicated pawn stats from the official server's data
//! tables (F-838, F-839), and the virtual-stat formulas a returning player's
//! restored stats add (F-984).
//!
//! The original sums every stat over its sources and derives the virtual
//! stats from integer formulas over other stats; the pawn replicates every
//! `bIsReplicated` `D_Stats` row whose value is nonzero, in row order. A
//! fresh player's only source is the starting grant (`D_CharacterStartingStats`
//! `Base_Stats`). The formulas below are written here by stat name for the
//! virtual stats that a fresh or a restored player's replicated stats depend
//! on; any other stat reads as its summed sources, which for a fresh player
//! matches the original only because no grant feeds another virtual stat. Enumerated
//! keys (tool types, atmospheres, times of day) are the original's enum
//! values, as its selections use them. Arithmetic wraps as the original's
//! 32-bit integers do.
use std::cell::RefCell;
use std::collections::BTreeMap;
use std::num::Wrapping;

use anyhow::{Context, Result, ensure};

use crate::data_pak::DataTables;
use crate::experimental::{pawn_open::FastArrayContent, rep_layout::StatPairRepState};

/// The stat table: a stat's ID is its row position (F-838).
pub const STATS_TABLE: &str = "D_Stats";
/// The row field marking a stat the pawn replicates.
pub const REPLICATED_FIELD: &str = "bIsReplicated";

/// Stat values from summed sources and the virtual-stat formulas.
pub struct StatValues<'a> {
    names: &'a [String],
    index: BTreeMap<&'a str, usize>,
    sources: BTreeMap<usize, i32>,
    cache: RefCell<BTreeMap<usize, i32>>,
}

impl<'a> StatValues<'a> {
    /// A container over the stat rows `names` with `sources` summed per
    /// stat row.
    pub fn new(names: &'a [String], sources: BTreeMap<usize, i32>) -> Self {
        Self {
            names,
            index: names
                .iter()
                .enumerate()
                .map(|(stat, name)| (name.as_str(), stat))
                .collect(),
            sources,
            cache: RefCell::new(BTreeMap::new()),
        }
    }

    /// The value of the stat at row `stat`: its formula's when it is a
    /// modelled virtual stat, else its summed sources.
    pub fn at(&self, stat: usize) -> i32 {
        if let Some(value) = self.cache.borrow().get(&stat) {
            return *value;
        }
        let name = self.names.get(stat).map(String::as_str);
        let value = match name.and_then(formula) {
            Some(formula) => formula(self).0,
            None => self.sources.get(&stat).copied().unwrap_or(0),
        };
        self.cache.borrow_mut().insert(stat, value);
        value
    }

    /// The value of the stat named `name`; a name the table lacks reads 0.
    pub fn value(&self, name: &str) -> i32 {
        self.index.get(name).map_or(0, |stat| self.at(*stat))
    }

    /// [`Self::value`] as a wrapping 32-bit integer, for the formulas.
    fn get(&self, name: &str) -> Wrapping<i32> {
        Wrapping(self.value(name))
    }

    /// `max(floor, (pct + 100) * base / 100)`, without a floor when `None`.
    fn scaled(&self, base: &str, pct: &str, floor: Option<i32>) -> Wrapping<i32> {
        let value = (self.get(pct) + HUNDRED) * self.get(base) / HUNDRED;
        floor.map_or(value, |floor| Wrapping(value.0.max(floor)))
    }

    /// `(coefficient * parent / 100 + base) * (pct + 100) / 100`.
    fn speed(&self, base: &str, parent: &str, coefficient: &str, pct: &str) -> Wrapping<i32> {
        (self.get(coefficient) * self.get(parent) / HUNDRED + self.get(base))
            * (self.get(pct) + HUNDRED)
            / HUNDRED
    }

    /// `stat` when the flag stat equals 1, else 0.
    fn when(&self, flag: &str, stat: &str) -> Wrapping<i32> {
        if self.value(flag) == 1 {
            self.get(stat)
        } else {
            Wrapping(0)
        }
    }

    /// `stat` when the flag stat is nonzero, else 0.
    fn nonzero(&self, flag: &str, stat: &str) -> Wrapping<i32> {
        if self.value(flag) != 0 {
            self.get(stat)
        } else {
            Wrapping(0)
        }
    }

    /// The stat `map` names for the enum value of `key`, or 0.
    fn select(&self, key: &str, map: &[(i32, &str)]) -> Wrapping<i32> {
        let key = self.value(key);
        map.iter()
            .find(|(value, _)| *value == key)
            .map_or(Wrapping(0), |(_, stat)| self.get(stat))
    }

    /// The `Base<biome><suffix>` stat of the current atmosphere, or 0.
    fn atmosphere(&self, suffix: &str) -> Wrapping<i32> {
        let key = self.value(ATMOSPHERE);
        ATMOSPHERE_BIOMES
            .iter()
            .find(|(value, _)| *value == key)
            .map_or(Wrapping(0), |(_, biome)| {
                self.get(&format!("Base{biome}{suffix}"))
            })
    }

    /// `trunc(value * scale * difference)` in single precision.
    fn insulated(&self, value: &str, scale: f32, difference: Wrapping<i32>) -> Wrapping<i32> {
        Wrapping((self.value(value) as f32 * scale * difference.0 as f32) as i32)
    }
}

const HUNDRED: Wrapping<i32> = Wrapping(100);

const TOOL_TYPE: &str = "ToolType_Enum";
const ATMOSPHERE: &str = "Atmosphere_Enum";
const TIME_OF_DAY: &str = "TimeOfDay_Enum";

/// The biome each atmosphere's selections name.
const ATMOSPHERE_BIOMES: [(i32, &str); 16] = [
    (0, "Cave"),
    (1, "Conifer"),
    (2, "Arctic"),
    (3, "Desert"),
    (4, "Swamp"),
    (5, "Volcanic"),
    (6, "Grasslands"),
    (8, "Swamp"),
    (9, "Arctic"),
    (10, "Cave"),
    (11, "Geothermal"),
    (12, "Coastal"),
    (13, "Tundra"),
    (15, "Desert"),
    (17, "Jungle"),
    (18, "Jungle"),
];

/// The item wear stats each time of day selects.
const ITEM_WEAR_TIMES: [(i32, &str); 2] =
    [(1, "BaseItemWearAtNight_+%"), (3, "BaseItemWearAtDay_+%")];

/// Tool types the original counts as firearms.
const FIREARMS: [i32; 3] = [10, 11, 12];

type Formula = fn(&StatValues<'_>) -> Wrapping<i32>;

/// The modelled virtual stat named `name`.
fn formula(name: &str) -> Option<Formula> {
    let formula: Formula = match name {
        // Pass-throughs of their base.
        "CharacterMass_+" => |s| s.get("BaseCharacterMass_+"),
        "HardenedPointDamageProtection_%" => |s| s.get("BaseHardenedPointDamageProtection_%"),
        "HungerLevel_+" => |s| s.get("BaseHungerLevel_+"),
        "ThirstLevel_+" => |s| s.get("BaseThirstLevel_+"),
        "InternalRestingTemperature_+" => |s| s.get("BaseInternalRestingTemperature_+"),
        "MaximumInternalTemperature_+" => |s| s.get("BaseMaximumInternalTemperature_+"),
        "MinimumInternalTemperature_+" => |s| s.get("BaseMinimumInternalTemperature_+"),
        "MaximumFallDamageVelocity_+" => |s| s.get("BaseMaximumFallDamageVelocity_+"),
        "MinimumFallDamageVelocity_+" => |s| s.get("BaseMinimumFallDamageVelocity_+"),
        "MaximumFallDamage_%" => |s| s.get("BaseMaximumFallDamage_%"),
        "MinimumFallDamage_%" => |s| s.get("BaseMinimumFallDamage_%"),
        "MaximumStomachFullness_+" => |s| s.get("BaseMaximumStomachFullness_+"),
        "MountedFoodConsumption_+%" => |s| s.get("BaseMountedFoodConsumption_+%"),
        "MountedOxygenConsumption_+%" => |s| s.get("BaseMountedOxygenConsumption_+%"),
        "MountedWaterConsumption_+%" => |s| s.get("BaseMountedWaterConsumption_+%"),
        "OxygenConsumedPerStaminaUsed_%" => |s| s.get("BaseOxygenConsumedPerStaminaUsed_+%"),
        "PartialStealthDamage_+%" => |s| s.get("BasePartialStealthDamage_+%"),
        "StealthDamage_+%" => |s| s.get("BaseStealthDamage_+%"),
        "RespawnDelay_+" => |s| s.get("BaseRespawnDelay_+"),
        "RespawnDistance_+" => |s| s.get("BaseRespawnDistance_+"),
        "StaminaConsumed_+%" => |s| s.get("BaseStaminaConsumed_+%"),
        "StaminaRegenDelay_+%" => |s| s.get("BaseStaminaRegenDelay_+%"),
        "StaminaRegenFromEmptyDelay_+%" => |s| s.get("BaseStaminaRegenFromEmptyDelay_+%"),
        // Bases scaled by a percentage, some floored.
        "MaximumHealth_+" => |s| s.scaled("BaseMaximumHealth_+", "BaseMaximumHealth_+%", Some(1)),
        "MaximumStamina_+" => {
            |s| s.scaled("BaseMaximumStamina_+", "BaseMaximumStamina_+%", Some(1))
        }
        "MaximumOxygen_+" => |s| s.scaled("BaseMaximumOxygen_+", "BaseMaximumOxygen_+%", None),
        "MaximumFood_+" => |s| s.scaled("BaseMaximumFood_+", "BaseMaximumFood_+%", None),
        "MaximumWater_+" => |s| s.scaled("BaseMaximumWater_+", "BaseMaximumWater_+%", None),
        "MaximumRadiation_+" => {
            |s| s.scaled("BaseMaximumRadiation_+", "BaseMaximumRadiation_+%", Some(1))
        }
        "WeightCapacity_+" => {
            |s| s.scaled("BaseWeightCapacity_+", "BaseWeightCapacity_+%", Some(1))
        }
        "CollisionDamageResistance_%" => |s| {
            s.scaled(
                "BaseCollisionDamageResistance_%",
                "BaseCollisionDamageResistance_+%",
                None,
            )
        },
        "MovementSpeed_+" => |s| s.scaled("BaseMovementSpeed_+", "MovementSpeed_+%", Some(0)),
        "OxygenConsumptionPerHour_+" => |s| {
            s.scaled(
                "BaseOxygenConsumptionPerHour_+",
                "OxygenConsumption_+%",
                Some(1),
            )
        },
        "FoodConsumptionPerHour_+" => |s| {
            s.scaled(
                "BaseFoodConsumptionPerHour_+",
                "FoodConsumption_+%",
                Some(1),
            )
        },
        "WaterConsumptionPerHour_+" => |s| {
            s.scaled(
                "BaseWaterConsumptionPerHour_+",
                "WaterConsumption_+%",
                Some(1),
            )
        },
        "HealthRegenPerMinute_+" => {
            |s| s.scaled("BaseHealthRegenPerMinute_+", "HealthRegen_+%", Some(0))
        }
        "StaminaRegenPerMinute_+" => {
            |s| s.scaled("BaseStaminaRegenPerMinute_+", "StaminaRegen_+%", Some(0))
        }
        "StaminaRegenDelay_+" => {
            |s| s.scaled("BaseStaminaRegenDelay_+", "StaminaRegenDelay_+%", None)
        }
        "StaminaRegenFromEmptyDelay_+" => |s| {
            s.scaled(
                "BaseStaminaRegenFromEmptyDelay_+",
                "StaminaRegenFromEmptyDelay_+%",
                None,
            )
        },
        // Movement speeds from movement speed and coefficients.
        "SprintSpeed_+" => |s| {
            s.speed(
                "BaseSprintSpeed_+",
                "MovementSpeed_+",
                "SprintMovementSpeedCoefficient_%",
                "BaseSprintSpeed_+%",
            )
        },
        "CrouchSpeed_+" => |s| {
            s.speed(
                "BaseCrouchSpeed_+",
                "MovementSpeed_+",
                "CrouchMovementSpeedCoefficient_%",
                "BaseCrouchSpeed_+%",
            )
        },
        "SwimSpeed_+" => |s| {
            s.speed(
                "BaseSwimSpeed_+",
                "MovementSpeed_+",
                "SwimMovementSpeedCoefficient_%",
                "BaseSwimSpeed_+%",
            )
        },
        "SwimSprintSpeed_+" => |s| {
            s.speed(
                "BaseSwimSprintSpeed_+",
                "SwimSpeed_+",
                "SwimSprintMovementSpeedCoefficient_%",
                "BaseSwimSprintSpeed_+%",
            )
        },
        "ClimbingSpeed_+" => |s| {
            s.speed(
                "BaseClimbingSpeed_+",
                "MovementSpeed_+",
                "ClimbMovementSpeedCoefficient_%",
                "BaseClimbingSpeed_+%",
            )
        },
        "ClimbingSprintSpeed_+" => |s| {
            s.speed(
                "BaseClimbingSprintSpeed_+",
                "ClimbingSpeed_+",
                "ClimbSprintMovementSpeedCoefficient_%",
                "BaseClimbingSprintSpeed_+%",
            )
        },
        "MovementSpeed_+%" => |s| {
            s.get("BaseMovementSpeed_+%")
                + s.get("OverencumberedMovementSpeed_+%")
                + s.get("LocationMovementSpeed_+%")
                + s.get("TimeMovementSpeed_+%")
                + s.get("CarryingMovementSpeed_+%")
                + s.get("FocusedItemMovementSpeed_+%")
                + s.nonzero("IsAiming_?", "AimingMovementSpeed_+%")
                + s.nonzero("IsInShallowWater_?", "BaseMovementSpeedInShallowWater_+%")
                + s.nonzero("IsPullingCart_?", "BaseMovementSpeedWhilePullingCart_+%")
                + s.nonzero("IsHarvestCartActive_?", "BaseCartActiveMovementSpeed_+%")
        },
        "OverencumberedMovementSpeed_+%" => |s| {
            (s.get("BaseOverencumberedPenalty_+%") + HUNDRED)
                * s.when("IsOverencumbered_?", "BaseOverencumberedMovementSpeed_+%")
                / HUNDRED
        },
        "LocationMovementSpeed_+%" => |s| s.atmosphere("MovementSpeed_+%"),
        "TimeMovementSpeed_+%" => |s| {
            s.when(
                "FinalMomentsOfDrop_?",
                "BaseFinalMomentsOfDropMovementSpeed_+%",
            ) + s.select(
                TIME_OF_DAY,
                &[
                    (2, "BaseMorningMovementSpeed_+%"),
                    (1, "BaseNightMovementSpeed_+%"),
                ],
            )
        },
        "CarryingMovementSpeed_+%" => |s| {
            s.nonzero("IsCarryingAnimal_?", "BaseAnimalCarryingMovementSpeed_+%")
                + s.nonzero("IsCarryingTool_?", "ToolCarryingMovementSpeed_+%")
                + s.nonzero("IsCarryingRadar_?", "BaseRadarCarryingMovementSpeed_+%")
                + s.nonzero("IsCarryingExotic_?", "BaseExoticCarryingMovementSpeed_+%")
                + s.nonzero(
                    "IsCarryingExtractor_?",
                    "BaseExtractorCarryingMovementSpeed_+%",
                )
        },
        "ToolCarryingMovementSpeed_+%" => |s| {
            s.get("BaseToolCarryingMovementSpeed_+%")
                + s.select(
                    TOOL_TYPE,
                    &[
                        (3, "BaseKnifeCarryingMovementSpeed_+%"),
                        (7, "BaseBowCarryingMovementSpeed_+%"),
                    ],
                )
        },
        "FocusedItemMovementSpeed_+%" => |s| {
            s.when("IsFirearm_?", "BaseFocusedFirearmMovementSpeed_+%")
                + s.select(
                    TOOL_TYPE,
                    &[
                        (7, "BaseFocusedBowMovementSpeed_+%"),
                        (8, "BaseFocusedFireWhackerMovementSpeed_+%"),
                    ],
                )
        },
        "AimingMovementSpeed_+%" => |s| {
            s.get("BaseAimingMovementSpeed_+%")
                + s.select(
                    TOOL_TYPE,
                    &[
                        (7, "BaseBowAimingMovementSpeed_+%"),
                        (3, "BaseKnifeAimingMovementSpeed_+%"),
                        (10, "BasePistolAimingMovementSpeed_+%"),
                        (11, "BaseRifleAimingMovementSpeed_+%"),
                        (12, "BaseShotgunAimingMovementSpeed_+%"),
                        (4, "BaseSpearAimingMovementSpeed_+%"),
                    ],
                )
        },
        "IsFirearm_?" => |s| Wrapping(i32::from(FIREARMS.contains(&s.value(TOOL_TYPE)))),
        "CriticalDamage_+%" => |s| {
            s.get("BaseCriticalDamage_+%")
                + s.select(
                    TOOL_TYPE,
                    &[
                        (7, "BaseBowCriticalDamage_+%"),
                        (4, "BaseSpearCriticalDamage_+%"),
                        (3, "BaseKnifeCriticalDamage_+%"),
                        (10, "BasePistolCriticalDamage_+%"),
                        (11, "BaseRifleCriticalDamage_+%"),
                        (12, "BaseShotgunCriticalDamage_+%"),
                    ],
                )
                + s.when("IsFirearm_?", "BaseFirearmCriticalDamage_+%")
        },
        // Survival rates by atmosphere, time of day and state.
        "HealthRegen_+%" => |s| s.get("BaseHealthRegen_+%") + s.atmosphere("HealthRegen_+%"),
        "StaminaRegen_+%" => |s| {
            s.get("BaseStaminaRegen_+%")
                + s.atmosphere("StaminaRegen_+%")
                + s.when("IsInStorm_?", "BaseStormStaminaRegen_+%")
                + s.when("IsWounded_?", "BaseWoundedStaminaRegen_+%")
                + s.select(
                    TOOL_TYPE,
                    &[
                        (4, "BaseCarryingSpearStaminaRegen_+%"),
                        (8, "BaseCarryingFirewhackerStaminaRegen_+%"),
                    ],
                )
        },
        "FoodConsumption_+%" => |s| {
            s.get("BaseFoodConsumption_+%")
                + s.select(TIME_OF_DAY, &[(1, "BaseNightFoodConsumption_+%")])
                + s.when("IsMounted_?", "MountedFoodConsumption_+%")
                + s.atmosphere("FoodConsumption_+%")
        },
        "OxygenConsumption_+%" => |s| {
            s.get("BaseOxygenConsumption_+%")
                + s.select(TIME_OF_DAY, &[(1, "BaseNightOxygenConsumption_+%")])
                + s.when("IsMounted_?", "MountedOxygenConsumption_+%")
                + s.when("IsSwimming_?", "BaseSwimmingOxygenConsumption_+%")
                + s.atmosphere("OxygenConsumption_+%")
        },
        "WaterConsumption_+%" => |s| {
            s.get("BaseWaterConsumption_+%")
                + s.select(TIME_OF_DAY, &[(1, "BaseNightWaterConsumption_+%")])
                + s.when("IsMounted_?", "MountedWaterConsumption_+%")
                + s.atmosphere("WaterConsumption_+%")
        },
        // Temperature.
        "ColdResistance_%" => |s| {
            Wrapping(
                (s.get("BaseColdResistance_%") + s.atmosphere("ColdResistance_+%"))
                    .0
                    .clamp(-50, 75),
            )
        },
        "HeatResistance_%" => |s| {
            Wrapping(
                (s.get("BaseHeatResistance_%") + s.atmosphere("HeatResistance_+%"))
                    .0
                    .clamp(-50, 75),
            )
        },
        "ArmorInsulation_%" => |s| Wrapping(s.value("BaseArmorInsulation_%").clamp(-100, 100)),
        "ColdInsulation_+" => |s| {
            s.insulated(
                "ArmorInsulation_%",
                0.01,
                s.get("BaseInternalTemperatureSafeRegionMaximum_+")
                    - s.get("BaseMaximumInternalTemperature_+"),
            )
        },
        "HeatInsulation_+" => |s| {
            s.insulated(
                "ArmorInsulation_%",
                0.01,
                s.get("BaseMinimumInternalTemperature_+")
                    - s.get("BaseInternalTemperatureSafeRegionMinimum_+"),
            )
        },
        "InternalTemperatureSafeRegionMaximum_+" => |s| {
            s.get("BaseInternalTemperatureSafeRegionMaximum_+")
                - s.insulated(
                    "HeatResistance_%",
                    0.01,
                    s.get("BaseInternalTemperatureSafeRegionMaximum_+")
                        - s.get("BaseMaximumInternalTemperature_+"),
                )
                - s.get("HeatInsulation_+")
        },
        "InternalTemperatureSafeRegionMinimum_+" => |s| {
            s.get("ColdInsulation_+")
                - s.insulated(
                    "ColdResistance_%",
                    -0.01,
                    s.get("BaseMinimumInternalTemperature_+")
                        - s.get("BaseInternalTemperatureSafeRegionMinimum_+"),
                )
                + s.get("BaseInternalTemperatureSafeRegionMinimum_+")
        },
        "CooldownDegreePerMinute_+" => |s| {
            let factor = match s.value("ThirstLevel_+") {
                1 => 50,
                2 => 0,
                _ => 100,
            };
            Wrapping(
                (s.get("BaseCooldownDegreePerMinute_+") * Wrapping(factor) / HUNDRED)
                    .0
                    .max(1),
            )
        },
        "WarmupDegreePerMinute_+" => |s| {
            let factor = match s.value("HungerLevel_+") {
                1 => 50,
                2 => 0,
                _ => 100,
            };
            Wrapping(
                (s.get("BaseWarmupDegreePerMinute_+") * Wrapping(factor) / HUNDRED)
                    .0
                    .max(1),
            )
        },
        "TreeCollisionDamageResistance_%" => |s| {
            Wrapping(
                (s.get("BaseTreeCollisionDamageResistance_%")
                    + s.get("CollisionDamageResistance_%"))
                .0
                .min(99),
            )
        },
        // A returning pawn's restored stats (F-984).
        "BackpackSlots_+" => |s| s.get("BaseBackpackSlots_+"),
        "UpgradeSlots_+" => |s| s.get("BaseUpgradeSlots_+"),
        "OxygenSlots_+" => |s| s.get("BaseOxygenSlots_+"),
        "WaterSlots_+" => |s| s.get("BaseWaterSlots_+"),
        "FoodStomachSlots_+" => |s| s.get("BaseFoodStomachSlots_+"),
        "BacterialModifierDuration_+%" => |s| s.get("BaseBacterialModifierDuration_+%"),
        "ParasiticModifierDuration_+%" => |s| s.get("BaseParasiticModifierDuration_+%"),
        "PoisonModifierDuration_+%" => |s| s.get("BasePoisonModifierDuration_+%"),
        "PhysicalTraumaModifierDuration_+%" => |s| s.get("BasePhysicalTraumaModifierDuration_+%"),
        "MountedExposureResistance_+%" => |s| s.get("BaseMountedExposureResistance_+%"),
        "FallDamageResistance_%" => |s| Wrapping(s.value("BaseFallDamageResistance_%").min(99)),
        "SprainModifierDuration_+%" => {
            |s| s.get("BaseSprainModifierDuration_+%") + s.get("BaseModifierDuration_+%")
        }
        "PhysicalDamageResistance_%" => |s| {
            let base = s.get("BasePhysicalDamageResistance_%")
                + s.select(
                    TOOL_TYPE,
                    &[(13, "BasePhysicalDamageResistanceWhileFishing_%")],
                );
            Wrapping(
                ((s.get("BasePhysicalDamageResistance_+%") + HUNDRED) * base / HUNDRED)
                    .0
                    .min(99),
            )
        },
        "MeleeDamageResistance_+%" => |s| {
            s.get("BaseMeleeDamageResistance_+%")
                + s.select(
                    TOOL_TYPE,
                    &[(4, "BaseCarryingSpearMeleeDamageResistance_+%")],
                )
        },
        "MeleeDamageResistance_%" => |s| {
            let base = s.get("BaseMeleeDamageResistance_%") + s.get("PhysicalDamageResistance_%");
            Wrapping(
                (base * (s.get("MeleeDamageResistance_+%") + HUNDRED) / HUNDRED)
                    .0
                    .min(99),
            )
        },
        "ProjectileDamageResistance_%" => |s| {
            Wrapping(
                (s.scaled(
                    "BaseProjectileDamageResistance_%",
                    "BaseProjectileDamageResistance_+%",
                    None,
                ) + s.get("PhysicalDamageResistance_%"))
                .0
                .min(99),
            )
        },
        "StaminaActionCost_+%" => |s| {
            s.get("BaseStaminaActionCost_+%")
                + s.when(
                    "IsOverencumbered_?",
                    "BaseOverEncumberedStaminaActionCost_+%",
                )
        },
        "JumpingStaminaActionCost_+%" => {
            |s| s.get("BaseJumpingStaminaActionCost_+%") + s.get("StaminaActionCost_+%")
        }
        "SprintingStaminaActionCost_+%" => {
            |s| s.get("BaseSprintingStaminaActionCost_+%") + s.get("StaminaActionCost_+%")
        }
        "ToolStaminaActionCost_+%" => |s| {
            s.get("StaminaActionCost_+%")
                + s.get("BaseToolStaminaActionCost_+%")
                + s.select(
                    TOOL_TYPE,
                    &[
                        (1, "BaseAxeStaminaActionCost_+%"),
                        (2, "BasePickaxeStaminaActionCost_+%"),
                        (3, "BaseKnifeStaminaActionCost_+%"),
                        (4, "BaseSpearStaminaActionCost_+%"),
                        (7, "BaseBowStaminaActionCost_+%"),
                    ],
                )
        },
        "ExposureResistance_+%" => |s| {
            s.get("BaseExposureResistance_+%")
                + s.atmosphere("ExposureResistance_+%")
                + s.when("IsMounted_?", "MountedExposureResistance_+%")
        },
        "ItemWear_+%" => |s| {
            s.get("BaseItemWear_+%")
                + s.select(TIME_OF_DAY, &ITEM_WEAR_TIMES)
                + s.select(
                    TOOL_TYPE,
                    &[
                        (1, "BaseAxeItemWear_+%"),
                        (2, "BasePickaxeItemWear_+%"),
                        (3, "BaseKnifeItemWear_+%"),
                        (5, "BaseSickleItemWear_+%"),
                        (7, "BaseBowItemWear_+%"),
                        (8, "BaseFirewhackerItemWear_+%"),
                        (9, "BaseRepairHammerItemWear_+%"),
                        (13, "BaseLureItemWear_+%"),
                    ],
                )
                + s.when("IsFirearm_?", "BaseFirearmItemWear_+%")
        },
        "LureItemWear_+%" => |s| {
            s.get("BaseItemWear_+%")
                + s.select(TIME_OF_DAY, &ITEM_WEAR_TIMES)
                + s.select(TOOL_TYPE, &[(13, "BaseLureItemWear_+%")])
        },
        _ => return None,
    };
    Some(formula)
}

/// The most stats a starting grant may name.
const MAX_GRANTS: usize = 4_096;

/// A fresh player's replicated stats (F-839): the starting grants of
/// `D_CharacterStartingStats` row `row`, evaluated through the formulas,
/// as every nonzero `bIsReplicated` `D_Stats` row in row order; the fast
/// array's replication key is its item count, as captured.
pub fn fresh_player_stats(
    tables: &mut DataTables,
    starting_table: &str,
    row: &str,
) -> Result<FastArrayContent<StatPairRepState>> {
    let names = tables.rows(STATS_TABLE)?.to_vec();
    let starting = tables
        .row(starting_table, row)?
        .with_context(|| format!("{starting_table} has no {row} row"))?;
    let granted = starting
        .get("StatsGranted")
        .and_then(serde_json::Value::as_object)
        .context("the starting stats grant no map")?;
    ensure!(
        granted.len() <= MAX_GRANTS,
        "the starting stats grant too many stats"
    );
    let mut sources = BTreeMap::new();
    for (key, value) in granted {
        // Keys are exported as `(Value="<stat row>")`.
        let name = key
            .strip_prefix("(Value=\"")
            .and_then(|rest| rest.strip_suffix("\")"))
            .context("a starting grant key is not a stat row handle")?;
        let stat = names
            .iter()
            .position(|row| row == name)
            .with_context(|| format!("{STATS_TABLE} lacks a granted stat"))?;
        let value = value
            .as_i64()
            .and_then(|value| i32::try_from(value).ok())
            .context("a starting grant is not an int")?;
        let summed: &mut i32 = sources.entry(stat).or_insert(0);
        *summed = summed
            .checked_add(value)
            .context("a starting grant overflows")?;
    }
    let values = StatValues::new(&names, sources);
    let mut items = Vec::new();
    for stat in 0..names.len() {
        if tables.row_bool(STATS_TABLE, REPLICATED_FIELD, stat)? != Some(true) {
            continue;
        }
        let value = values.at(stat);
        if value != 0 {
            items.push(StatPairRepState {
                stat: u32::try_from(stat)?,
                value,
            });
        }
    }
    Ok(FastArrayContent {
        replication_key: i32::try_from(items.len())?,
        items,
    })
}
