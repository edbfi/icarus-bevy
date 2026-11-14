//! Independently authored session bootstrap; no terrain or gameplay data.
use crate::experimental::{
    archive::ByteOrder,
    game_state_open::handle,
    manager_open::{ManagerOpenPlan, ManagerSlot, compose_manager_open},
    release::ReleaseProfile,
    rep_layout::RepValue,
    session_open::{ConnectionExports, NetGuidCache},
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Explicit world inputs. Tile references do not create tile actors or geometry.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthoredWorldBootstrap {
    pub seed: i32,
    pub seed_initialised: bool,
    pub flod_tile_count: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clock: Option<AuthoredClockConfig>,
}

impl AuthoredWorldBootstrap {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (0..=65535).contains(&self.flod_tile_count),
            "authored world FLOD tile count must be 0..=65535"
        );
        if let Some(clock) = &self.clock {
            clock.prepare()?;
        }
        Ok(())
    }

    /// Reject mixing explicit bootstrap state with a preserved prospect.
    pub fn validate_preserved_prospect(&self, present: bool) -> Result<()> {
        ensure!(
            !present,
            "network.authored_world conflicts with a preserved prospect"
        );
        Ok(())
    }
    pub fn plan(
        &self,
        profile: &ReleaseProfile,
        map: &str,
    ) -> Result<(BTreeMap<u32, RepValue>, ManagerOpenPlan)> {
        self.validate()?;
        profile
            .game_state_open_plan(map)?
            .context("authored world requires a GameState plan")?;
        ensure!(
            !profile.manager_open_plans(map)?.is_empty(),
            "authored world requires initial manager plans"
        );
        let flod = profile
            .manager_open_plan(ManagerSlot::Flod, map)?
            .context("release has no FLOD manager plan")?
            .with_flod_tiles(self.flod_tile_count)?;
        // The array's u16 count alone does not establish that the manager open
        // fits the entry and payload budgets. This early rejection uses fresh
        // GUID/export state; it is not a guarantee for every live-cache encoding.
        // Runtime composition remains authoritative for the assigned GUIDs/channel.
        for order in [ByteOrder::Little, ByteOrder::Big] {
            compose_manager_open(
                &mut NetGuidCache::new(),
                &mut ConnectionExports::new(),
                &flod,
                ManagerSlot::Flod.key(),
                6,
                order,
            )
            .context("authored FLOD manager exceeds the wire codec limits")?;
        }
        Ok((
            BTreeMap::from([
                (handle::SEED, RepValue::Int(self.seed)),
                (
                    handle::SEED_INITIALISED,
                    RepValue::Bool(self.seed_initialised),
                ),
            ]),
            flod,
        ))
    }
}

/// Uniform independently authored game time. This is not the original world curve.
#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct AuthoredClockConfig {
    pub initial_minute: u16,
    pub seconds_per_day: i32,
    pub tick_hz: u32,
}

/// Immutable bounded rational phase; the persisted simulation tick is its only clock.
#[derive(Clone, Copy, Debug)]
pub struct AuthoredClockPlan {
    initial_minute: u16,
    seconds_per_day: i32,
    tick_hz: u32,
    ticks_per_day: u64,
}

impl AuthoredClockConfig {
    pub fn prepare(&self) -> Result<AuthoredClockPlan> {
        ensure!(
            self.initial_minute < 1440,
            "authored clock initial_minute must be 0..=1439"
        );
        ensure!(
            self.seconds_per_day > 0,
            "authored clock seconds_per_day must be positive"
        );
        ensure!(
            (1..=240).contains(&self.tick_hz),
            "authored clock tick_hz must be 1..=240"
        );
        let ticks_per_day = u64::try_from(self.seconds_per_day)?
            .checked_mul(u64::from(self.tick_hz))
            .context("authored clock period overflow")?;
        Ok(AuthoredClockPlan {
            initial_minute: self.initial_minute,
            seconds_per_day: self.seconds_per_day,
            tick_hz: self.tick_hz,
            ticks_per_day,
        })
    }
}

impl AuthoredClockPlan {
    /// Sample the last completed simulation tick, with no wall-time advance or replay.
    /// Final binary32 values that round to midnight are canonical positive zero.
    pub fn sample(&self, tick: u64) -> f32 {
        let period = u128::from(self.ticks_per_day);
        let numerator = (u128::from(self.initial_minute) * period
            + 1440 * u128::from(tick % self.ticks_per_day))
            % (1440 * period);
        let minutes = (numerator as f64 / period as f64) as f32;
        if minutes == 0.0 || minutes >= 1440.0 {
            0.0
        } else {
            minutes
        }
    }
    pub fn seconds_per_day(&self) -> i32 {
        self.seconds_per_day
    }
    pub fn tick_hz(&self) -> u32 {
        self.tick_hz
    }
}

#[cfg(test)]
mod authored_clock_tests {
    use super::*;

    fn config() -> AuthoredClockConfig {
        AuthoredClockConfig {
            initial_minute: 480,
            seconds_per_day: 3600,
            tick_hz: 30,
        }
    }

    #[test]
    fn rational_phase_has_declared_initial_partial_midnight_and_multi_day_values() {
        let plan = config().prepare().unwrap();
        assert_eq!(plan.sample(0), 480.0);
        assert_eq!(plan.sample(30 * 900), 840.0);
        assert_eq!(plan.sample(30 * 2400).to_bits(), 0.0f32.to_bits());
        assert_eq!(plan.sample(30 * 3600 * 10), 480.0);
        assert_eq!(plan.sample(30 * 3600 * 10 + 30 * 900), 840.0);
        assert_eq!(plan.seconds_per_day(), 3600);
        assert_eq!(plan.tick_hz(), 30);
    }

    #[test]
    fn large_tick_and_maximum_period_are_bounded_without_replay() {
        for declaration in [
            config(),
            AuthoredClockConfig {
                initial_minute: 1439,
                seconds_per_day: i32::MAX,
                tick_hz: 240,
            },
            AuthoredClockConfig {
                initial_minute: 0,
                seconds_per_day: 1,
                tick_hz: 1,
            },
        ] {
            let plan = declaration.prepare().unwrap();
            let period =
                u64::from(declaration.seconds_per_day as u32) * u64::from(declaration.tick_hz);
            let phase = u64::MAX % period;
            assert_eq!(
                plan.sample(u64::MAX).to_bits(),
                plan.sample(phase).to_bits()
            );
            for tick in [0, 1, u64::MAX / 2, u64::MAX] {
                let sample = plan.sample(tick);
                assert!(sample.is_finite() && (0.0..1440.0).contains(&sample));
                assert!(!sample.is_sign_negative());
            }
        }
    }

    #[test]
    fn binary32_rounding_at_midnight_canonicalizes_to_positive_zero() {
        let plan = AuthoredClockConfig {
            initial_minute: 0,
            seconds_per_day: i32::MAX,
            tick_hz: 240,
        }
        .prepare()
        .unwrap();
        let period = u64::from(i32::MAX as u32) * 240;
        // One tick before midnight is close enough to round to 1440 in f32.
        assert_eq!(plan.sample(period - 1).to_bits(), 0.0f32.to_bits());
        // A quarter minute earlier remains on the preceding day.
        let before = plan.sample(period - period / 5760);
        assert_eq!(before, 1439.75);
        assert_eq!(plan.sample(period).to_bits(), 0.0f32.to_bits());
    }

    #[test]
    fn declaration_bounds_and_unknown_members_refuse() {
        for bad in [
            AuthoredClockConfig {
                initial_minute: 1440,
                ..config()
            },
            AuthoredClockConfig {
                seconds_per_day: 0,
                ..config()
            },
            AuthoredClockConfig {
                seconds_per_day: -1,
                ..config()
            },
            AuthoredClockConfig {
                tick_hz: 0,
                ..config()
            },
            AuthoredClockConfig {
                tick_hz: 241,
                ..config()
            },
        ] {
            assert!(bad.prepare().is_err());
        }
        assert!(
            serde_json::from_str::<AuthoredClockConfig>(
                r#"{"initial_minute":0,"seconds_per_day":1,"tick_hz":1,"future":true}"#
            )
            .is_err()
        );
    }

    #[test]
    fn omitted_clock_preserves_exact_existing_recipe_bytes() {
        let legacy = br#"{"seed":42,"seed_initialised":true,"flod_tile_count":0}"#;
        let world: AuthoredWorldBootstrap = serde_json::from_slice(legacy).unwrap();
        assert!(world.clock.is_none());
        assert_eq!(serde_json::to_vec(&world).unwrap().as_slice(), legacy);
        let with_clock = AuthoredWorldBootstrap {
            clock: Some(config()),
            ..world
        };
        assert_ne!(serde_json::to_vec(&with_clock).unwrap().as_slice(), legacy);
        assert_eq!(
            serde_json::from_slice::<AuthoredWorldBootstrap>(
                &serde_json::to_vec(&with_clock).unwrap()
            )
            .unwrap(),
            with_clock
        );
    }
}
