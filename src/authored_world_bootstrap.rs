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
}

impl AuthoredWorldBootstrap {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (0..=65535).contains(&self.flod_tile_count),
            "authored world FLOD tile count must be 0..=65535"
        );
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
