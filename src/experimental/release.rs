//! EXPERIMENTAL pin and data-driven release profile of the official release this
//! server answers to.
//!
//! ICARUS ships weekly, and each release changes the project version string
//! that the client hashes into its Hello network version (F-541, F-685); the
//! original server compares that value for exact equality and answers a
//! mismatch with Upgrade (F-563). Server code, gameplay-tag tables and paks
//! change between weekly builds too (F-686, F-700, F-702), so per-release
//! protocol metadata is isolated in [`ReleaseProfile`]: `config/release.toml`
//! holds the active pin and profile parameters, and `tools/release_watch.py`
//! moves or diffs it when Steam's public branch moves.
//! Nothing here claims compatibility with the pinned build.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use super::{
    default_version_string_checksum,
    game_state_open::{GameStateComponents, GameStateOpenPlan},
    manager_open::{
        ACTOR_STATE_CURRENT_BIOME, ManagerComponent, ManagerOpenPlan, ManagerSlot,
        STAT_CONTAINER_HANDLE,
    },
    pawn_open::{PawnComponent, PawnContentPlan, PawnOpenPlan},
    rep_layout::{
        ICARUS_ROW_HANDLE_NONE, ICARUS_ROW_HANDLE_TABLE_COUNT, ICARUS_TWO_BYTE_ROW_TABLES,
        ICARUS_ZERO_ROW_TABLES, MovementLevel, RepValue, RotationLevel, TagEncoding,
        actor_component_rep_layout, actor_rep_layout, actor_state_rep_layout,
        bp_icarus_player_character_survival_rep_layout, bp_icarus_player_state_rep_layout,
        icarus_game_state_survival_rep_layout, icarus_player_controller_survival_rep_layout,
        inventory_rep_layout, inventory_slot_commands, player_character_state_rep_layout,
        quest_manager_rep_layout, radiation_manager_rep_layout,
        weather_manager_component_rep_layout, world_talent_manager_rep_layout,
    },
    rep_rpc::{
        BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
        BP_ICARUS_PLAYER_STATE_FIELD_INDEX_BOUND, BP_MOUNT_BASE_FIELD_INDEX_BOUND,
        BP_MOUNT_HORSE_FIELD_INDEX_BOUND, BP_TAME_BASE_FIELD_INDEX_BOUND,
        INVENTORY_FIELD_INDEX_BOUND, STAT_CONTAINER_FIELD_INDEX_BOUND,
        UI_PROJECTION_MOUNT_STATUS_FIELD_INDEX_BOUND,
    },
    session_open::{
        COMPONENT_HANDLES, ComponentBlock, ComponentExport, PLAYER_CONTROLLER_CHANNEL_INDEX,
        PLAYER_STATE_CHANNEL_INDEX, PathSegment, PlayerControllerOpenPlan, PlayerControllerProfile,
        PlayerStateOpenPlan, map_level_chain,
    },
};

/// Dedicated server Steam AppID.
pub const DEDICATED_SERVER_APP_ID: u32 = 2_089_300;
/// Image project name the version string starts with (F-541).
pub const PROJECT_NAME: &str = "Icarus";
/// Network changelist without an override (F-542).
pub const NETWORK_CHANGELIST: u32 = 0;
/// Engine network protocol version (F-541).
pub const ENGINE_NETWORK_VERSION: u32 = 17;
/// Game network protocol version, zero-filled (F-541).
pub const GAME_NETWORK_VERSION: u32 = 0;

/// Observed gameplay-tag net-index hash (`NetworkGameplayTagNodeIndexHash`,
/// F-702) for 3.0.31 (`build_id = 25793607`, 2,299 unique sorted nodes).
pub const TAG_NET_INDEX_HASH_3_0_31: u32 = 0xdc78_0a27;
/// Observed gameplay-tag net-index hash (`NetworkGameplayTagNodeIndexHash`,
/// F-702) for 3.0.32 (`build_id = 25956137`, 2,302 unique sorted nodes).
pub const TAG_NET_INDEX_HASH_3_0_32: u32 = 0x6d0f_676f;

/// The string the engine hashes into the Hello network version.
pub fn network_version_string(project_version: &str) -> String {
    format!(
        "{PROJECT_NAME} {project_version}, NetCL: {NETWORK_CHANGELIST}, \
         EngineNetVer: {ENGINE_NETWORK_VERSION}, GameNetVer: {GAME_NETWORK_VERSION}"
    )
}

/// The Hello network version a client of `project_version` sends when no
/// override delegate is bound (F-541).
pub fn network_checksum(project_version: &str) -> u32 {
    default_version_string_checksum(&network_version_string(project_version))
}

/// One official release, as `config/release.toml` pins it.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleasePin {
    /// Steam app of the dedicated server.
    pub app_id: u32,
    /// Steam build of the public branch.
    pub build_id: u64,
    /// The project version the server logs and the client hashes.
    pub project_version: String,
    /// `network_checksum(project_version)`, stored so a reviewer sees the
    /// value a client sends.
    pub network_checksum: u32,
}

/// Per-release gameplay-tag net-index metadata (`UGameplayTagsManager`, F-702).
///
/// [`ReleaseTagProfile::tag_encoding`] supplies the wire [`TagEncoding`] used by
/// `examples/capture_decode.rs`; `data_table_tag_count`, `ini_tag_count`,
/// `unique_node_count`, `configured_first_segment`, `container_bits`, and
/// `net_index_hash` are descriptive release-qualification metadata validated by
/// [`ReleaseTagProfile::validate`] (the `rep_layout` container layouts currently
/// use `GAMEPLAY_TAG_CONTAINER_COUNT_BITS` directly).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseTagProfile {
    /// Declared tag rows in `Data/Tags/D_Tags` (130 in 3.0.31/32, F-702);
    /// descriptive qualification count.
    pub data_table_tag_count: u32,
    /// Declared `GameplayTagList` entries in `DefaultGameplayTags.ini`
    /// (1,886 in 3.0.31; 1,889 in 3.0.32, F-702); descriptive qualification count.
    pub ini_tag_count: u32,
    /// Unique sorted tag nodes after dot-prefix expansion (`NetIndexTrueBitNum`
    /// input: 2,299 in 3.0.31; 2,302 in 3.0.32, F-702).
    pub unique_node_count: u16,
    /// `NetIndexTrueBitNum` (`1..=16`; 12 in 3.0.31/32, F-702).
    pub total_bits: u8,
    /// Configured `NetIndexFirstBitSegment` before clamping (`0..=16`; 16 in
    /// `DefaultGameplayTags.ini`, F-702); descriptive qualification metadata.
    pub configured_first_segment: u8,
    /// Effective `first_segment` passed to [`TagEncoding::NetIndex`] (`0` or
    /// `>= total_bits` selects unsplit `total_bits` packing, F-591, F-702).
    pub effective_first_segment: u8,
    /// `NumBitsForContainerSize` (`1..=8`; 6 in 3.0.31/32, F-702); descriptive
    /// qualification metadata validated against `1..=8`.
    pub container_bits: u8,
    /// Chained lowercase UTF-16LE CRC-32 (`NetworkGameplayTagNodeIndexHash`,
    /// F-702); descriptive qualification hash.
    pub net_index_hash: u32,
}

impl ReleaseTagProfile {
    /// Validate tag widths and node count against the net-index codec bounds.
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=16).contains(&self.total_bits),
            "tag total_bits must be 1..=16"
        );
        ensure!(
            self.configured_first_segment <= 16 && self.effective_first_segment <= 16,
            "tag first_segment must be 0..=16"
        );
        ensure!(
            (1..=8).contains(&self.container_bits),
            "tag container_bits must be 1..=8"
        );
        let invalid_sentinel = u32::from(self.unique_node_count) + 1;
        ensure!(
            invalid_sentinel < (1_u32 << self.total_bits),
            "tag total_bits {} cannot encode invalid sentinel {} for {} nodes",
            self.total_bits,
            invalid_sentinel,
            self.unique_node_count
        );
        let expected_bits = (invalid_sentinel + 1)
            .next_power_of_two()
            .trailing_zeros()
            .max(1) as u8;
        ensure!(
            self.total_bits == expected_bits,
            "tag total_bits {} does not match derived Ceillog2 width {} for {} nodes",
            self.total_bits,
            expected_bits,
            self.unique_node_count
        );
        Ok(())
    }

    /// Return the wire [`TagEncoding`] for `RepCommand::GameplayTag` and
    /// `RepCommand::GameplayTagContainer`.
    pub const fn tag_encoding(&self) -> TagEncoding {
        TagEncoding::NetIndex {
            first_segment: self.effective_first_segment,
            total_bits: self.total_bits,
        }
    }
}

/// Per-release `FClassNetCache` `SerializeInt` field-index bounds
/// (`FieldsBase + Fields.Num() + 1`, F-644, F-705, F-708).
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseClassNetCacheBounds {
    /// `UIcarusStatContainer` (`2 + 1 + 1 + 1 = 5`, F-705, F-708).
    pub stat_container: u32,
    /// `UInventory` (`2 + 1 + 6 + 1 + 1 = 11`, F-708).
    pub inventory: u32,
    /// `BP_UIProjectionComponent_MountStatus_C` (`2 + 2 + 1 + 1 + 1 = 7`, F-708).
    pub ui_projection_mount_status: u32,
    /// `BP_IcarusPlayerState_C` (`10 + 0 + 11 + 6 + 10 + 1 = 38`, F-708).
    pub bp_icarus_player_state: u32,
    /// `BP_Mount_Base_C` (`10 + 3 + 17 + 25 + 6 + 6 + 10 + 6 + 1 = 84`, F-708).
    pub bp_mount_base: u32,
    /// `BP_Mount_Horse_C` (`83 + 1 + 1 = 85`, F-708).
    pub bp_mount_horse: u32,
    /// `BP_Tame_Base_C` (`83 + 2 + 1 = 86`, F-708).
    pub bp_tame_base: u32,
    /// `BP_IcarusPlayerControllerSurvival_C`
    /// (`10 + 4 + 74 + 13 + 29 + 25 + 27 + 1 = 183`, F-708).
    pub bp_icarus_player_controller_survival: u32,
}

impl Default for ReleaseClassNetCacheBounds {
    fn default() -> Self {
        Self {
            stat_container: STAT_CONTAINER_FIELD_INDEX_BOUND,
            inventory: INVENTORY_FIELD_INDEX_BOUND,
            ui_projection_mount_status: UI_PROJECTION_MOUNT_STATUS_FIELD_INDEX_BOUND,
            bp_icarus_player_state: BP_ICARUS_PLAYER_STATE_FIELD_INDEX_BOUND,
            bp_mount_base: BP_MOUNT_BASE_FIELD_INDEX_BOUND,
            bp_mount_horse: BP_MOUNT_HORSE_FIELD_INDEX_BOUND,
            bp_tame_base: BP_TAME_BASE_FIELD_INDEX_BOUND,
            bp_icarus_player_controller_survival:
                BP_ICARUS_PLAYER_CONTROLLER_SURVIVAL_FIELD_INDEX_BOUND,
        }
    }
}

impl ReleaseClassNetCacheBounds {
    pub fn validate(&self) -> Result<()> {
        for (name, bound) in [
            ("stat_container", self.stat_container),
            ("inventory", self.inventory),
            (
                "ui_projection_mount_status",
                self.ui_projection_mount_status,
            ),
            ("bp_icarus_player_state", self.bp_icarus_player_state),
            ("bp_mount_base", self.bp_mount_base),
            ("bp_mount_horse", self.bp_mount_horse),
            ("bp_tame_base", self.bp_tame_base),
            (
                "bp_icarus_player_controller_survival",
                self.bp_icarus_player_controller_survival,
            ),
        ] {
            ensure!(bound >= 1, "FClassNetCache bound {name} must be at least 1");
        }
        Ok(())
    }

    /// Look up the actor RPC `FClassNetCache` bound for `schema`.
    pub fn actor_bound(&self, schema: &str) -> Option<u32> {
        match schema {
            "BP_IcarusPlayerState_C" => Some(self.bp_icarus_player_state),
            "BP_Mount_Base_C" => Some(self.bp_mount_base),
            "BP_Mount_Horse_C" => Some(self.bp_mount_horse),
            "BP_Tame_Base_C" => Some(self.bp_tame_base),
            "AIcarusPlayerControllerSurvival" => Some(self.bp_icarus_player_controller_survival),
            _ => None,
        }
    }
}

/// Per-release `FRowHandle::NetSerialize` (`0x143f8a530`, F-705) table-width
/// profile from `SetupTables`.
///
/// Descriptive release-qualification metadata validated against the static
/// `SetupTables` oracle; the low-level `rep_layout` `FRowHandle` reader and
/// writer currently consult `ICARUS_*_ROW_TABLES` directly rather than taking a
/// runtime [`ReleaseRowHandleProfile`].
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRowHandleProfile {
    /// Total static data-table names registered in `SetupTables` (`293`, F-705).
    pub table_count: u16,
    /// Sorted table indices whose packaged `DataTable` has 0 rows (0-byte row index).
    pub zero_row_tables: Vec<u16>,
    /// Sorted table indices whose packaged `DataTable` has 255..=65534 rows
    /// (2-byte row index).
    pub two_byte_row_tables: Vec<u16>,
}

impl Default for ReleaseRowHandleProfile {
    fn default() -> Self {
        Self {
            table_count: ICARUS_ROW_HANDLE_TABLE_COUNT,
            zero_row_tables: ICARUS_ZERO_ROW_TABLES.to_vec(),
            two_byte_row_tables: ICARUS_TWO_BYTE_ROW_TABLES.to_vec(),
        }
    }
}

impl ReleaseRowHandleProfile {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.table_count >= 1 && self.table_count < ICARUS_ROW_HANDLE_NONE,
            "row-handle table_count must be 1..65535"
        );
        for (label, list) in [
            ("zero_row_tables", &self.zero_row_tables),
            ("two_byte_row_tables", &self.two_byte_row_tables),
        ] {
            for window in list.windows(2) {
                ensure!(
                    window[0] < window[1],
                    "{label} must be strictly ascending without duplicates"
                );
            }
            if let Some(&last) = list.last() {
                ensure!(
                    last < self.table_count,
                    "{label} index {last} exceeds table_count {}",
                    self.table_count
                );
            }
        }
        for index in &self.zero_row_tables {
            ensure!(
                self.two_byte_row_tables.binary_search(index).is_err(),
                "table index {index} appears in both zero_row_tables and two_byte_row_tables"
            );
        }
        Ok(())
    }

    /// Row-index byte width (`0`, `1`, `2`) for `table_index`.
    ///
    /// Descriptive helper matching [`super::rep_layout::icarus_row_handle_width_bytes`];
    /// the low-level `rep_layout` codec still reads the static
    /// `ICARUS_*_ROW_TABLES` tables directly.
    pub fn row_width_bytes(&self, table_index: u16) -> Result<u8> {
        if table_index == ICARUS_ROW_HANDLE_NONE {
            return Ok(0);
        }
        ensure!(
            table_index < self.table_count,
            "unknown row handle data table index {table_index}"
        );
        if self.zero_row_tables.binary_search(&table_index).is_ok() {
            Ok(0)
        } else if self.two_byte_row_tables.binary_search(&table_index).is_ok() {
            Ok(2)
        } else {
            Ok(1)
        }
    }
}

/// One segment of the PlayerController archetype's outer chain, leaf first.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleasePathSegment {
    pub path: String,
    /// Class network checksum (`F-776`) for the archetype; `0` for its package.
    pub checksum: u32,
    #[serde(default)]
    pub no_load: bool,
}

/// What a PlayerController component contributes to the open body (`F-778`).
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseComponentBlockKind {
    /// Exported and referenced only; no content block.
    #[default]
    None,
    /// A stably named subobject block without properties.
    Create,
    /// A stably named subobject block carrying `rep_bools`.
    RepBools,
}

/// One replicated PlayerController component, in serialization order.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseComponent {
    pub name: String,
    /// Class network checksum of the component's class (`F-776`).
    pub checksum: u32,
    #[serde(default)]
    pub block: ReleaseComponentBlockKind,
    /// `[handle, value]` pairs of `UActorComponent` bool handles (`1..=2`),
    /// strictly ascending; required exactly when `block = "rep_bools"`.
    #[serde(default)]
    pub rep_bools: Vec<(u32, bool)>,
}

/// Per-release identifiers of the PlayerController open (`[player_controller]`,
/// `F-778`, `F-779`): the archetype chain and the replicated components with
/// their class network checksums. The level chain is map-dependent and is
/// supplied by the caller.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleasePlayerControllerProfile {
    pub archetype: Vec<ReleasePathSegment>,
    pub components: Vec<ReleaseComponent>,
}

/// An archetype outer chain: the archetype with its class network checksum,
/// then its package with checksum `0`, all nonempty ASCII paths.
fn validate_archetype(section: &str, archetype: &[ReleasePathSegment]) -> Result<()> {
    ensure!(
        archetype.len() >= 2,
        "{section} archetype needs the archetype and its package"
    );
    for segment in archetype {
        ensure!(
            segment.path.is_ascii() && !segment.path.is_empty(),
            "{section} archetype paths must be nonempty ASCII"
        );
    }
    ensure!(
        archetype[0].checksum != 0,
        "{section} archetype needs its class network checksum"
    );
    ensure!(
        archetype[archetype.len() - 1].checksum == 0,
        "{section} archetype package carries checksum 0"
    );
    Ok(())
}

fn session_segments(archetype: &[ReleasePathSegment]) -> Vec<PathSegment> {
    archetype
        .iter()
        .map(|segment| PathSegment::new(&segment.path, segment.checksum, segment.no_load))
        .collect()
}

/// Per-release identifiers of the world manager opens (`[managers]`,
/// F-859): each manager's archetype chain and the component classes'
/// checksums. Values are the managers' defaults.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseManagersProfile {
    pub quest_manager: Vec<ReleasePathSegment>,
    pub radiation_manager: Vec<ReleasePathSegment>,
    pub world_talent_manager: Vec<ReleasePathSegment>,
    pub disaster_controller: Vec<ReleasePathSegment>,
    /// `UIcarusStatContainer` and `UActorState` network checksums.
    pub stat_container_checksum: u32,
    pub actor_state_checksum: u32,
}

impl ReleaseManagersProfile {
    /// The managers reproduced bit-exactly against the 3.0.31 capture
    /// (F-859, F-861). For 3.0.32 the six checksums, including the Blueprint
    /// `BP_DisasterController_C`'s, are among the values the 25956137
    /// derivation reproduces unchanged (M-820, behind F-780); the component
    /// set and order are assumed unchanged.
    pub fn icarus_survival() -> Self {
        let native = |class: &str, checksum| {
            vec![
                ReleasePathSegment {
                    path: format!("Default__{class}"),
                    checksum,
                    no_load: false,
                },
                ReleasePathSegment {
                    path: "/Script/Icarus".to_owned(),
                    checksum: 0,
                    no_load: false,
                },
            ]
        };
        Self {
            quest_manager: native("QuestManager", 3_539_440_926),
            radiation_manager: native("RadiationManager", 2_158_376_861),
            world_talent_manager: native("WorldTalentManager", 3_510_503_235),
            disaster_controller: vec![
                ReleasePathSegment {
                    path: "Default__BP_DisasterController_C".to_owned(),
                    checksum: 2_281_024_597,
                    no_load: false,
                },
                ReleasePathSegment {
                    path: "/Game/BP/Systems/Disaster/BP_DisasterController".to_owned(),
                    checksum: 0,
                    no_load: false,
                },
            ],
            stat_container_checksum: 4_037_894_903,
            actor_state_checksum: 455_559_420,
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_archetype("managers quest_manager", &self.quest_manager)?;
        validate_archetype("managers radiation_manager", &self.radiation_manager)?;
        validate_archetype("managers world_talent_manager", &self.world_talent_manager)?;
        validate_archetype("managers disaster_controller", &self.disaster_controller)
    }
}

/// Per-release identifiers of the PlayerState open (`[player_state]`, F-784):
/// its archetype chain. Values come from the session and admission.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleasePlayerStateProfile {
    pub archetype: Vec<ReleasePathSegment>,
}

impl ReleasePlayerStateProfile {
    /// `BP_IcarusPlayerState_C`: reproduced bit-exactly against the 3.0.31
    /// capture and derived unchanged for 3.0.32 (F-780, F-784).
    pub fn icarus_survival() -> Self {
        Self {
            archetype: vec![
                ReleasePathSegment {
                    path: "Default__BP_IcarusPlayerState_C".to_owned(),
                    checksum: 628_225_127,
                    no_load: false,
                },
                ReleasePathSegment {
                    path: "/Game/BP/Systems/BP_IcarusPlayerState".to_owned(),
                    checksum: 0,
                    no_load: false,
                },
            ],
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_archetype("player_state", &self.archetype)
    }
}

/// A replicated component of the GameState or the pawn: its name and class
/// network checksum (`F-776`).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSubobject {
    pub name: String,
    pub checksum: u32,
}

/// Per-release identifiers of the GameState open (`[game_state]`, F-794):
/// the archetype, game mode class and spectator class chains, and the three
/// replicated components. Values come from the session.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseGameStateProfile {
    pub archetype: Vec<ReleasePathSegment>,
    pub game_mode_class: Vec<ReleasePathSegment>,
    pub spectator_class: Vec<ReleasePathSegment>,
    pub connected_players: ReleaseSubobject,
    pub dialogue_system: ReleaseSubobject,
    pub weather: ReleaseSubobject,
}

impl ReleaseGameStateProfile {
    /// `BP_IcarusGameState_C`: reproduced bit-exactly against the 3.0.31
    /// capture (F-794). For 3.0.32 the six checksums are among the 77 oracle
    /// values the 25956137 derivation reproduces (M-820, behind F-780); the
    /// component set and order are assumed unchanged.
    pub fn icarus_survival() -> Self {
        let segment = |path: &str, checksum| ReleasePathSegment {
            path: path.to_owned(),
            checksum,
            no_load: false,
        };
        let subobject = |name: &str, checksum| ReleaseSubobject {
            name: name.to_owned(),
            checksum,
        };
        Self {
            archetype: vec![
                segment("Default__BP_IcarusGameState_C", 2_614_124_130),
                segment("/Game/BP/Systems/BP_IcarusGameState", 0),
            ],
            game_mode_class: vec![
                segment("BP_IcarusGameMode_C", 2_281_024_597),
                segment("/Game/BP/Systems/BP_IcarusGameMode", 0),
            ],
            spectator_class: vec![
                segment("SpectatorPawn", 3_712_879_475),
                segment("/Script/Engine", 0),
            ],
            connected_players: subobject("ConnectedPlayers", 3_014_652_130),
            dialogue_system: subobject("DialogueSystem", 3_400_199_742),
            weather: subobject("WeatherComp", 3_554_357_950),
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_archetype("game_state", &self.archetype)?;
        validate_archetype("game_state game_mode_class", &self.game_mode_class)?;
        validate_archetype("game_state spectator_class", &self.spectator_class)?;
        let mut names = std::collections::BTreeSet::new();
        for component in [
            &self.connected_players,
            &self.dialogue_system,
            &self.weather,
        ] {
            let name = &component.name;
            ensure!(
                name.is_ascii() && !name.is_empty(),
                "game_state component names must be nonempty ASCII"
            );
            ensure!(
                names.insert(name.to_ascii_lowercase()),
                "duplicate game_state component {name}"
            );
            ensure!(component.checksum != 0, "component {name} needs a checksum");
        }
        Ok(())
    }

    fn component(subobject: &ReleaseSubobject) -> ComponentExport {
        ComponentExport {
            name: subobject.name.clone(),
            checksum: subobject.checksum,
            block: ComponentBlock::None,
        }
    }
}

/// One replicated pawn component, in block order: its name, class network
/// checksum (`F-776`) and the `UActorComponent` bools its block carries at
/// open.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleasePawnComponent {
    pub name: String,
    pub checksum: u32,
    /// `[handle, value]` pairs of handles `1..=2`, strictly ascending.
    #[serde(default)]
    pub rep_bools: Vec<(u32, bool)>,
}

/// Per-release identifiers of the player's pawn open (`[pawn]`, F-797): the
/// archetype chain, the replicated components in block order, the
/// components `StatContainer`, `ActorState` and `InventoryComponent` name,
/// and the runtime inventories' class chain. Values come from the session.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleasePawnProfile {
    pub archetype: Vec<ReleasePathSegment>,
    pub components: Vec<ReleasePawnComponent>,
    /// The component `StatContainer` (handle 47) names.
    pub stat_container: String,
    /// The component `ActorState` (handle 48) names.
    pub actor_state: String,
    /// The component `InventoryComponent` (handle 56) names.
    pub inventory_component: String,
    /// `Inventory` with its class network checksum, then `/Script/Icarus`.
    pub inventory_class: Vec<ReleasePathSegment>,
}

impl ReleasePawnProfile {
    /// `BP_IcarusPlayerCharacterSurvival_C`: reproduced bit-exactly against the
    /// 3.0.31 capture (F-797), with the component bools and the inventory
    /// class it carries. For 3.0.32 the archetype, component and `Inventory`
    /// checksums are among the 77 oracle values the 25956137 derivation
    /// reproduces (M-820, behind F-780); the component set, order and bools
    /// are assumed unchanged.
    pub fn icarus_survival() -> Self {
        let segment = |path: &str, checksum| ReleasePathSegment {
            path: path.to_owned(),
            checksum,
            no_load: false,
        };
        let subobject = |name: &str, checksum| ReleasePawnComponent {
            name: name.to_owned(),
            checksum,
            rep_bools: Vec::new(),
        };
        let active = |name: &str, checksum| ReleasePawnComponent {
            name: name.to_owned(),
            checksum,
            rep_bools: vec![(2, true)],
        };
        Self {
            archetype: vec![
                segment("Default__BP_IcarusPlayerCharacterSurvival_C", 430_507_263),
                segment("/Game/BP/Player/BP_IcarusPlayerCharacterSurvival", 0),
            ],
            components: vec![
                subobject("Stat Container", 4_037_894_903),
                active("InventoryComponent", 46_421_488),
                subobject("AudioContext", 4_273_702_982),
                subobject("FeedbackAudioComponent", 3_400_199_742),
                subobject("ActorState", 692_782_745),
                active("Stomach", 3_911_258_249),
                active("Processing", 1_023_342_657),
                active("BP_Weight", 46_421_488),
                active("BP_SwimmingComponent", 165_351_633),
                active("BP_ShelteredComponent", 2_435_860_737),
                subobject("BP_PlayerBuildingPlacement", 1_301_726_856),
                active("Experience", 46_421_488),
                subobject("BP_PlayerMovementAudioComponent", 1_588_614_535),
                subobject("BP_PlayerMusicComponent", 681_416_933),
                active("BP_Flammable_Player", 3_465_019_412),
                subobject("BP_UIProjectionComponent_Player", 1_863_489_512),
            ],
            stat_container: "Stat Container".to_owned(),
            actor_state: "ActorState".to_owned(),
            inventory_component: "InventoryComponent".to_owned(),
            inventory_class: vec![
                segment("Inventory", 1_798_236_809),
                segment("/Script/Icarus", 0),
            ],
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_archetype("pawn", &self.archetype)?;
        ensure!(!self.components.is_empty(), "pawn needs its components");
        let mut names = std::collections::BTreeSet::new();
        for component in &self.components {
            let name = &component.name;
            ensure!(
                name.is_ascii() && !name.is_empty(),
                "pawn component names must be nonempty ASCII"
            );
            ensure!(
                names.insert(name.to_ascii_lowercase()),
                "duplicate pawn component {name}"
            );
            ensure!(component.checksum != 0, "component {name} needs a checksum");
            validate_rep_bools(name, &component.rep_bools)?;
        }
        validate_archetype("pawn inventory_class", &self.inventory_class)?;
        let [stat_container, actor_state, inventory_component] = self.referenced()?;
        ensure!(
            stat_container != actor_state
                && stat_container != inventory_component
                && actor_state != inventory_component,
            "pawn stat_container, actor_state and inventory_component must name distinct components"
        );
        Ok(())
    }

    /// Indices into `components` of `stat_container`, `actor_state` and
    /// `inventory_component`.
    fn referenced(&self) -> Result<[usize; 3]> {
        let index = |field: &str, name: &str| {
            self.components
                .iter()
                .position(|component| component.name == name)
                .with_context(|| format!("pawn {field} {name:?} is not a pawn component"))
        };
        Ok([
            index("stat_container", &self.stat_container)?,
            index("actor_state", &self.actor_state)?,
            index("inventory_component", &self.inventory_component)?,
        ])
    }
}

/// `UActorComponent`'s two bool handles (F-703).
const ACTOR_COMPONENT_BOOL_HANDLES: std::ops::RangeInclusive<u32> = 1..=2;

/// `rep_bools` name `UActorComponent` bool handles, strictly ascending.
fn validate_rep_bools(name: &str, rep_bools: &[(u32, bool)]) -> Result<()> {
    for (handle, _) in rep_bools {
        ensure!(
            ACTOR_COMPONENT_BOOL_HANDLES.contains(handle),
            "component {name}: rep_bools handle {handle} is not a UActorComponent bool"
        );
    }
    for window in rep_bools.windows(2) {
        ensure!(
            window[0].0 < window[1].0,
            "component {name}: rep_bools handles must be strictly ascending"
        );
    }
    Ok(())
}

impl ReleasePlayerControllerProfile {
    /// `BP_IcarusPlayerControllerSurvival_C` identifiers: reproduced bit-exactly
    /// against the 3.0.31 capture (`F-779`) and derived unchanged for 3.0.32
    /// from its own binary and paks (`F-780`).
    pub fn icarus_survival() -> Self {
        let segment = |path: &str, checksum| ReleasePathSegment {
            path: path.to_owned(),
            checksum,
            no_load: false,
        };
        let component = |name: &str, checksum, block, rep_bools: &[(u32, bool)]| ReleaseComponent {
            name: name.to_owned(),
            checksum,
            block,
            rep_bools: rep_bools.to_vec(),
        };
        use ReleaseComponentBlockKind::{Create, None, RepBools};
        Self {
            archetype: vec![
                segment(
                    "Default__BP_IcarusPlayerControllerSurvival_C",
                    3_911_321_073,
                ),
                segment("/Game/BP/Player/BP_IcarusPlayerControllerSurvival", 0),
            ],
            components: vec![
                component(
                    "BackendProxyComponent",
                    4_100_406_609,
                    RepBools,
                    &[(2, true)],
                ),
                component("PlayerDataComponent", 3_400_199_742, None, &[]),
                component("BestiaryManagerComponent", 2_451_266_650, Create, &[]),
                component(
                    "ResourceNetworkDataRequesterComponent",
                    3_500_113_941,
                    Create,
                    &[],
                ),
                component("BP_CriticalHitComponent", 1_900_637_040, Create, &[]),
                component("BP_HuntingManager", 2_334_391_351, Create, &[]),
                component(
                    "BP_NetworkProxyComponent",
                    997_381_320,
                    RepBools,
                    &[(2, true)],
                ),
            ],
        }
    }

    pub fn validate(&self) -> Result<()> {
        validate_archetype("player_controller", &self.archetype)?;
        ensure!(
            self.components.len() >= COMPONENT_HANDLES,
            "player_controller needs at least {COMPONENT_HANDLES} components"
        );
        let mut names = std::collections::BTreeSet::new();
        for component in &self.components {
            let name = &component.name;
            ensure!(
                name.is_ascii() && !name.is_empty(),
                "player_controller component names must be nonempty ASCII"
            );
            ensure!(
                names.insert(name.to_ascii_lowercase()),
                "duplicate player_controller component {name}"
            );
            ensure!(component.checksum != 0, "component {name} needs a checksum");
            ensure!(
                (component.block == ReleaseComponentBlockKind::RepBools)
                    != component.rep_bools.is_empty(),
                "component {name}: rep_bools must be given exactly when block = \"rep_bools\""
            );
            validate_rep_bools(name, &component.rep_bools)?;
        }
        Ok(())
    }

    /// The `session_open` profile for this release and the caller's level chain.
    pub fn session_profile(&self, level: Vec<PathSegment>) -> PlayerControllerProfile {
        PlayerControllerProfile {
            archetype: session_segments(&self.archetype),
            level,
            components: self
                .components
                .iter()
                .map(|component| ComponentExport {
                    name: component.name.clone(),
                    checksum: component.checksum,
                    block: match component.block {
                        ReleaseComponentBlockKind::None => ComponentBlock::None,
                        ReleaseComponentBlockKind::Create => ComponentBlock::CreateOnly,
                        ReleaseComponentBlockKind::RepBools => {
                            ComponentBlock::RepBools(component.rep_bools.clone())
                        }
                    },
                })
                .collect(),
        }
    }
}

/// Explicit, data-driven protocol profile for one ICARUS release (`docs/architecture.md#release-tracking`).
///
/// `pin`, `engine_network_version`, `tags.tag_encoding()`, and `class_net_cache`
/// drive [`super::peer_lab::PeerLabConfig::with_release_profile`] and
/// `examples/capture_decode.rs`; `game_network_version`, `row_handles`, and the
/// remaining `tags` fields record descriptive release-qualification metadata.
/// `player_controller` feeds [`super::session_open`]; it is `None` for builds
/// without a built-in or explicit `[player_controller]` section. Likewise
/// `player_state`, `game_state` and `pawn` feed the PlayerState, GameState
/// and pawn open plans and are `None` without a built-in or explicit
/// `[player_state]`, `[game_state]` or `[pawn]` section.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReleaseProfile {
    pub pin: ReleasePin,
    pub engine_network_version: u32,
    /// Descriptive game network protocol version (`0` in observed builds, F-541).
    pub game_network_version: u32,
    pub tags: ReleaseTagProfile,
    pub class_net_cache: ReleaseClassNetCacheBounds,
    pub row_handles: ReleaseRowHandleProfile,
    pub player_controller: Option<ReleasePlayerControllerProfile>,
    pub player_state: Option<ReleasePlayerStateProfile>,
    pub game_state: Option<ReleaseGameStateProfile>,
    pub pawn: Option<ReleasePawnProfile>,
    pub managers: Option<ReleaseManagersProfile>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleasePinFile {
    release: ReleasePin,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseFile {
    release: ReleasePin,
    #[serde(default)]
    tags: Option<ReleaseTagProfile>,
    #[serde(default)]
    class_net_cache: Option<ReleaseClassNetCacheBounds>,
    #[serde(default)]
    row_handles: Option<ReleaseRowHandleProfile>,
    #[serde(default)]
    player_controller: Option<ReleasePlayerControllerProfile>,
    #[serde(default)]
    player_state: Option<ReleasePlayerStateProfile>,
    #[serde(default)]
    game_state: Option<ReleaseGameStateProfile>,
    #[serde(default)]
    pawn: Option<ReleasePawnProfile>,
    #[serde(default)]
    managers: Option<ReleaseManagersProfile>,
}

impl ReleasePin {
    /// Parse a pin file and check its stored checksum.
    pub fn parse(text: &str) -> Result<Self> {
        let parsed = toml::from_str::<ReleasePinFile>(text).context("release pin")?;
        parsed.release.validate()?;
        Ok(parsed.release)
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path).with_context(|| path.display().to_string())?)
    }

    fn validate(&self) -> Result<()> {
        ensure!(
            self.project_version.is_ascii() && !self.project_version.is_empty(),
            "project_version must be nonempty ASCII"
        );
        let expected = network_checksum(&self.project_version);
        ensure!(
            self.network_checksum == expected,
            "network_checksum {} does not match {expected} for {}",
            self.network_checksum,
            self.project_version
        );
        Ok(())
    }
}

impl ReleaseProfile {
    /// Built-in profile for the 3.0.31 reference capture build (`25793607`
    /// server / `25793767` client, `3.0.31.158478-rel-DangerousHorizons`,
    /// F-689, F-702..F-708).
    pub fn release_3_0_31() -> Self {
        let project_version = "3.0.31.158478-rel-DangerousHorizons".to_owned();
        let network_checksum = network_checksum(&project_version);
        Self {
            pin: ReleasePin {
                app_id: DEDICATED_SERVER_APP_ID,
                build_id: 25_793_607,
                project_version,
                network_checksum,
            },
            engine_network_version: ENGINE_NETWORK_VERSION,
            game_network_version: GAME_NETWORK_VERSION,
            tags: ReleaseTagProfile {
                data_table_tag_count: 130,
                ini_tag_count: 1_886,
                unique_node_count: 2_299,
                total_bits: 12,
                configured_first_segment: 16,
                effective_first_segment: 0,
                container_bits: 6,
                net_index_hash: TAG_NET_INDEX_HASH_3_0_31,
            },
            class_net_cache: ReleaseClassNetCacheBounds::default(),
            row_handles: ReleaseRowHandleProfile::default(),
            player_controller: Some(ReleasePlayerControllerProfile::icarus_survival()),
            player_state: Some(ReleasePlayerStateProfile::icarus_survival()),
            game_state: Some(ReleaseGameStateProfile::icarus_survival()),
            pawn: Some(ReleasePawnProfile::icarus_survival()),
            managers: Some(ReleaseManagersProfile::icarus_survival()),
        }
    }

    /// Built-in profile for the pinned 3.0.32 release (`25956137`,
    /// `3.0.32.158743-rel-DangerousHorizons`, F-700, F-702..F-708).
    pub fn release_3_0_32() -> Self {
        let project_version = "3.0.32.158743-rel-DangerousHorizons".to_owned();
        let network_checksum = network_checksum(&project_version);
        Self {
            pin: ReleasePin {
                app_id: DEDICATED_SERVER_APP_ID,
                build_id: 25_956_137,
                project_version,
                network_checksum,
            },
            engine_network_version: ENGINE_NETWORK_VERSION,
            game_network_version: GAME_NETWORK_VERSION,
            tags: ReleaseTagProfile {
                data_table_tag_count: 130,
                ini_tag_count: 1_889,
                unique_node_count: 2_302,
                total_bits: 12,
                configured_first_segment: 16,
                effective_first_segment: 0,
                container_bits: 6,
                net_index_hash: TAG_NET_INDEX_HASH_3_0_32,
            },
            class_net_cache: ReleaseClassNetCacheBounds::default(),
            row_handles: ReleaseRowHandleProfile::default(),
            player_controller: Some(ReleasePlayerControllerProfile::icarus_survival()),
            player_state: Some(ReleasePlayerStateProfile::icarus_survival()),
            game_state: Some(ReleaseGameStateProfile::icarus_survival()),
            pawn: Some(ReleasePawnProfile::icarus_survival()),
            managers: Some(ReleaseManagersProfile::icarus_survival()),
        }
    }

    /// Built-in tag profile for a qualified build ID (`25793607`/`25793767` for
    /// 3.0.31, `25956137` for 3.0.32). Any other build ID requires an explicit
    /// `[tags]` section in the TOML profile so a weekly pin update cannot
    /// silently inherit an older build's tag metadata.
    fn default_tags_for_build(build_id: u64) -> Result<ReleaseTagProfile> {
        match build_id {
            25_793_607 | 25_793_767 => Ok(Self::release_3_0_31().tags),
            25_956_137 => Ok(Self::release_3_0_32().tags),
            _ => bail!(
                "no built-in tag profile for build_id {build_id}; supply an explicit [tags] section"
            ),
        }
    }

    /// Built-in PlayerController identifiers for a qualified build ID; `None`
    /// elsewhere, so a pin bump never inherits an older build's checksums.
    fn default_player_controller_for_build(
        build_id: u64,
    ) -> Option<ReleasePlayerControllerProfile> {
        match build_id {
            25_793_607 | 25_793_767 | 25_956_137 => {
                Some(ReleasePlayerControllerProfile::icarus_survival())
            }
            _ => None,
        }
    }

    /// Built-in PlayerState identifiers for a qualified build ID; `None`
    /// elsewhere, so a pin bump never inherits an older build's checksums.
    fn default_player_state_for_build(build_id: u64) -> Option<ReleasePlayerStateProfile> {
        match build_id {
            25_793_607 | 25_793_767 | 25_956_137 => {
                Some(ReleasePlayerStateProfile::icarus_survival())
            }
            _ => None,
        }
    }

    /// Built-in manager identifiers for a qualified build ID; `None`
    /// elsewhere, so a pin bump never inherits an older build's checksums.
    fn default_managers_for_build(build_id: u64) -> Option<ReleaseManagersProfile> {
        match build_id {
            25_793_607 | 25_793_767 | 25_956_137 => Some(ReleaseManagersProfile::icarus_survival()),
            _ => None,
        }
    }

    /// Built-in GameState identifiers for a qualified build ID; `None`
    /// elsewhere, so a pin bump never inherits an older build's checksums.
    fn default_game_state_for_build(build_id: u64) -> Option<ReleaseGameStateProfile> {
        match build_id {
            25_793_607 | 25_793_767 | 25_956_137 => {
                Some(ReleaseGameStateProfile::icarus_survival())
            }
            _ => None,
        }
    }

    /// Built-in pawn identifiers for a qualified build ID; `None` elsewhere,
    /// so a pin bump never inherits an older build's checksums.
    fn default_pawn_for_build(build_id: u64) -> Option<ReleasePawnProfile> {
        match build_id {
            25_793_607 | 25_793_767 | 25_956_137 => Some(ReleasePawnProfile::icarus_survival()),
            _ => None,
        }
    }

    /// Validate all release-profile invariants.
    pub fn validate(&self) -> Result<()> {
        self.pin.validate()?;
        ensure!(
            self.engine_network_version >= 1,
            "engine_network_version must be at least 1"
        );
        self.tags.validate()?;
        self.class_net_cache.validate()?;
        self.row_handles.validate()?;
        if let Some(player_controller) = &self.player_controller {
            player_controller.validate()?;
        }
        if let Some(player_state) = &self.player_state {
            player_state.validate()?;
        }
        if let Some(game_state) = &self.game_state {
            game_state.validate()?;
        }
        if let Some(pawn) = &self.pawn {
            pawn.validate()?;
        }
        if let Some(managers) = &self.managers {
            managers.validate()?;
        }
        Ok(())
    }

    /// Parse a release TOML file (`[release]` plus optional `[tags]`,
    /// `[class_net_cache]`, `[row_handles]`, `[player_controller]`,
    /// `[player_state]`, `[game_state]` and `[pawn]` sections) and validate
    /// it.
    pub fn parse(text: &str) -> Result<Self> {
        let parsed = toml::from_str::<ReleaseFile>(text).context("release profile")?;
        let tags = match parsed.tags {
            Some(tags) => tags,
            None => Self::default_tags_for_build(parsed.release.build_id)?,
        };
        let class_net_cache = parsed.class_net_cache.unwrap_or_default();
        let row_handles = parsed.row_handles.unwrap_or_default();
        let player_controller = parsed
            .player_controller
            .or_else(|| Self::default_player_controller_for_build(parsed.release.build_id));
        let player_state = parsed
            .player_state
            .or_else(|| Self::default_player_state_for_build(parsed.release.build_id));
        let game_state = parsed
            .game_state
            .or_else(|| Self::default_game_state_for_build(parsed.release.build_id));
        let pawn = parsed
            .pawn
            .or_else(|| Self::default_pawn_for_build(parsed.release.build_id));
        let managers = parsed
            .managers
            .or_else(|| Self::default_managers_for_build(parsed.release.build_id));
        let profile = Self {
            pin: parsed.release,
            engine_network_version: ENGINE_NETWORK_VERSION,
            game_network_version: GAME_NETWORK_VERSION,
            tags,
            class_net_cache,
            row_handles,
            player_controller,
            player_state,
            game_state,
            pawn,
            managers,
        };
        profile.validate()?;
        Ok(profile)
    }

    /// The PlayerController open plan for the map package of `map_url` and a
    /// server-chosen spawn location, or `None` without `[player_controller]`.
    /// Rep layouts are the statically observed `AIcarusPlayerControllerSurvival`
    /// and `UActorComponent` layouts (F-703, F-704) with this release's tag
    /// encoding.
    pub fn player_controller_open_plan(
        &self,
        map_url: &str,
        spawn_location: [f32; 3],
    ) -> Result<Option<PlayerControllerOpenPlan>> {
        let Some(player_controller) = &self.player_controller else {
            return Ok(None);
        };
        Ok(Some(PlayerControllerOpenPlan {
            profile: player_controller.session_profile(map_level_chain(map_url)?),
            actor_layout: icarus_player_controller_survival_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
                self.tags.tag_encoding(),
            ),
            component_layout: actor_component_rep_layout(),
            engine_net_version: self.engine_network_version,
            spawn_location,
            channel_index: PLAYER_CONTROLLER_CHANNEL_INDEX,
        }))
    }

    /// The PlayerState open plan for the map package of `map_url`, or `None`
    /// without `[player_state]`. The layout is the cooked
    /// `BP_IcarusPlayerState_C` layout (F-705) with the movement levels the
    /// capture decodes with.
    pub fn player_state_open_plan(&self, map_url: &str) -> Result<Option<PlayerStateOpenPlan>> {
        let Some(player_state) = &self.player_state else {
            return Ok(None);
        };
        Ok(Some(PlayerStateOpenPlan {
            archetype: session_segments(&player_state.archetype),
            level: map_level_chain(map_url)?,
            layout: bp_icarus_player_state_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
            ),
            engine_net_version: self.engine_network_version,
            channel_index: PLAYER_STATE_CHANNEL_INDEX,
        }))
    }

    /// The world manager open plans for the map package of `map_url`, or
    /// none without `[managers]` (F-859): the radiation and world-talent
    /// managers carry a stat container and an actor state (`CurrentBiome`
    /// row 0, as captured), the disaster controller a stat container only.
    /// The QuestManager is [`Self::quest_manager_open_plan`]'s.
    pub fn manager_open_plans(&self, map_url: &str) -> Result<Vec<(ManagerSlot, ManagerOpenPlan)>> {
        Ok(self
            .all_manager_open_plans(map_url)?
            .into_iter()
            .filter(|(slot, _)| *slot != ManagerSlot::Quest)
            .collect())
    }

    /// The QuestManager open plan for the map package of `map_url`, or
    /// `None` without `[managers]` (F-861): a stat container and an actor
    /// state like the radiation manager's, without quest state, which
    /// [`ManagerOpenPlan::with_quest_state`] adds from a save.
    pub fn quest_manager_open_plan(&self, map_url: &str) -> Result<Option<ManagerOpenPlan>> {
        Ok(self
            .all_manager_open_plans(map_url)?
            .into_iter()
            .find_map(|(slot, plan)| (slot == ManagerSlot::Quest).then_some(plan)))
    }

    fn all_manager_open_plans(&self, map_url: &str) -> Result<Vec<(ManagerSlot, ManagerOpenPlan)>> {
        let Some(managers) = &self.managers else {
            return Ok(Vec::new());
        };
        let level = map_level_chain(map_url)?;
        let (location, velocity, rotation) = (
            MovementLevel::Whole,
            MovementLevel::Whole,
            RotationLevel::Bytes,
        );
        let tags = self.tags.tag_encoding();
        let icarus_components = || {
            vec![
                ManagerComponent::stat_container(
                    managers.stat_container_checksum,
                    Some(STAT_CONTAINER_HANDLE),
                ),
                ManagerComponent::actor_state(
                    managers.actor_state_checksum,
                    actor_state_rep_layout(),
                    BTreeMap::from([(ACTOR_STATE_CURRENT_BIOME, RepValue::Byte(0))]),
                ),
            ]
        };
        let plan = |archetype: &[ReleasePathSegment], layout, components| ManagerOpenPlan {
            archetype: session_segments(archetype),
            level: level.clone(),
            layout,
            components,
            values: BTreeMap::new(),
            actors: BTreeMap::new(),
            keyed_actors: BTreeMap::new(),
            engine_net_version: self.engine_network_version,
        };
        Ok(vec![
            (
                ManagerSlot::Quest,
                plan(
                    &managers.quest_manager,
                    quest_manager_rep_layout(location, velocity, rotation, tags),
                    icarus_components(),
                ),
            ),
            (
                ManagerSlot::WorldTalent,
                plan(
                    &managers.world_talent_manager,
                    world_talent_manager_rep_layout(location, velocity, rotation, tags),
                    icarus_components(),
                ),
            ),
            (
                ManagerSlot::Radiation,
                plan(
                    &managers.radiation_manager,
                    radiation_manager_rep_layout(location, velocity, rotation, tags),
                    icarus_components(),
                ),
            ),
            (
                ManagerSlot::Disaster,
                plan(
                    &managers.disaster_controller,
                    actor_rep_layout(location, velocity, rotation),
                    vec![ManagerComponent::stat_container(
                        managers.stat_container_checksum,
                        None,
                    )],
                ),
            ),
        ])
    }

    /// The GameState open plan for the map package of `map_url`, or `None`
    /// without `[game_state]`. The layouts are the statically observed
    /// `AIcarusGameStateSurvival` and `UWeatherManagerComponent` layouts
    /// (F-703) with the movement levels the capture decodes with.
    pub fn game_state_open_plan(&self, map_url: &str) -> Result<Option<GameStateOpenPlan>> {
        let Some(game_state) = &self.game_state else {
            return Ok(None);
        };
        Ok(Some(GameStateOpenPlan {
            archetype: session_segments(&game_state.archetype),
            level: map_level_chain(map_url)?,
            game_mode_class: session_segments(&game_state.game_mode_class),
            spectator_class: session_segments(&game_state.spectator_class),
            components: GameStateComponents {
                connected_players: ReleaseGameStateProfile::component(
                    &game_state.connected_players,
                ),
                dialogue_system: ReleaseGameStateProfile::component(&game_state.dialogue_system),
                weather: ReleaseGameStateProfile::component(&game_state.weather),
            },
            layout: icarus_game_state_survival_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
            ),
            weather_layout: weather_manager_component_rep_layout(),
            engine_net_version: self.engine_network_version,
        }))
    }

    /// The pawn open plan for the map package of `map_url`, or `None` without
    /// `[pawn]`. The layouts are the statically observed
    /// `BP_IcarusPlayerCharacterSurvival_C` layout with the movement levels
    /// the capture decodes with, and the `UActorComponent`,
    /// `UPlayerCharacterState`, `UInventory` and `FInventorySlot` layouts,
    /// with this release's tag encoding and field bounds (F-797, F-708).
    pub fn pawn_open_plan(&self, map_url: &str) -> Result<Option<PawnOpenPlan>> {
        let Some(pawn) = &self.pawn else {
            return Ok(None);
        };
        let [stat_container, actor_state, inventory_component] = pawn.referenced()?;
        Ok(Some(PawnOpenPlan {
            archetype: session_segments(&pawn.archetype),
            level: map_level_chain(map_url)?,
            components: pawn
                .components
                .iter()
                .map(|component| PawnComponent {
                    name: component.name.clone(),
                    checksum: component.checksum,
                    rep_bools: component.rep_bools.clone(),
                })
                .collect(),
            stat_container,
            actor_state,
            inventory_component,
            layout: bp_icarus_player_character_survival_rep_layout(
                MovementLevel::Hundredths,
                MovementLevel::Whole,
                RotationLevel::Bytes,
                self.tags.tag_encoding(),
            ),
            content: PawnContentPlan {
                component_layout: actor_component_rep_layout(),
                character_state_layout: player_character_state_rep_layout(),
                inventory_class: session_segments(&pawn.inventory_class),
                inventory_layout: inventory_rep_layout(),
                slot_layout: inventory_slot_commands(self.tags.tag_encoding()),
                stat_container_bound: self.class_net_cache.stat_container,
                inventory_bound: self.class_net_cache.inventory,
            },
            engine_net_version: self.engine_network_version,
        }))
    }

    /// Load and validate a release profile from disk.
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path).with_context(|| path.display().to_string())?)
    }
}
