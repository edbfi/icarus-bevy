//! Shared Run/observer prerequisites for the bounded protected proof case.
use anyhow::{Result, ensure};
use icarus_bevy::{
    config::ServerConfig, experimental::pawn_open::CharacterVitals,
    native_player_runtime::ConsumptionConfig,
};

pub(super) fn validate_config(config: &ServerConfig) -> Result<()> {
    ensure!(
        config.network.player_depletion_damage.is_none(),
        "private leave proof refuses depletion damage"
    );
    ensure!(
        config.network.player_inventory.is_none() && !config.network.player_inventory_transfers,
        "private leave proof requires schema 14"
    );
    ensure!(
        config.network.enabled
            && config.network.authored_controller_leave
            && config.network.player_persistence.is_some()
            && config.network.hosting.is_some()
            && config.network.max_players == 1
            && config.network.join_password.is_set()
            && config.network.player_consumption
                == Some(ConsumptionConfig {
                    oxygen: 0,
                    water: 0,
                    food: 1,
                }),
        "private proof configuration refused"
    );
    Ok(())
}

pub(super) fn validate_initial_vitals(vitals: CharacterVitals) -> Result<()> {
    // At most 100 changing food debits plus the fixed receipt chain fit the
    // 128-event journal. Phase two may reopen a fully consumed checkpoint;
    // this bound checks the unchanged fresh recipe, not the loaded record.
    ensure!(
        (1..=100).contains(&vitals.food) && (vitals.food..=10_000_000).contains(&vitals.max_food),
        "private proof initial food refused"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipe_food_bound_is_positive_valid_and_journal_safe() {
        let mut vitals = CharacterVitals {
            health: 100,
            max_health: 100,
            stamina: 100,
            max_stamina: 100,
            oxygen: 100,
            max_oxygen: 100,
            water: 100,
            max_water: 100,
            food: 100,
            max_food: 100,
            biome_row: 0,
            growth_row: 0,
        };
        for food in [1, 100] {
            vitals.food = food;
            vitals.max_food = food;
            assert!(validate_initial_vitals(vitals).is_ok());
        }
        for food in [0, 101, i32::MAX] {
            vitals.food = food;
            vitals.max_food = food;
            assert!(validate_initial_vitals(vitals).is_err());
        }
        for (food, max_food) in [(99, 100), (100, 200)] {
            vitals.food = food;
            vitals.max_food = max_food;
            assert!(validate_initial_vitals(vitals).is_ok());
        }
        vitals.food = 100;
        vitals.max_food = 99;
        assert!(validate_initial_vitals(vitals).is_err());
    }
}

#[cfg(test)]
mod inventory_refusal_tests {
    use super::*;
    #[test]
    fn schema15_recipe_is_refused_before_any_proof_or_service_setup() {
        let mut config = ServerConfig::default();
        config.network.player_inventory = Some("unused-authored-recipe.json".into());
        assert_eq!(
            validate_config(&config).unwrap_err().to_string(),
            "private leave proof requires schema 14"
        );
    }
    #[test]
    fn transfer_policy_is_refused_even_without_a_recipe() {
        for recipe in [None, Some("unused-authored-recipe.json".into())] {
            let mut config = ServerConfig::default();
            config.network.player_inventory = recipe;
            config.network.player_inventory_transfers = true;
            assert_eq!(
                validate_config(&config).unwrap_err().to_string(),
                "private leave proof requires schema 14"
            );
        }
    }
}

#[cfg(test)]
mod depletion_refusal_tests {
    use super::*;
    #[test]
    fn depletion_damage_is_refused_before_proof_setup() {
        let mut config = ServerConfig::default();
        config.network.player_depletion_damage =
            Some(icarus_bevy::native_player_runtime::DepletionDamageConfig {
                oxygen: 0,
                water: 0,
                food: 1,
            });
        assert_eq!(
            validate_config(&config).unwrap_err().to_string(),
            "private leave proof refuses depletion damage"
        );
    }
}
