//! EXPERIMENTAL pin and data-driven release profile of the official release this
//! server answers to.
//!
//! ICARUS ships weekly, and each release changes the project version string
//! that the client hashes into its Hello network version (F-541, F-685); the
//! original server compares that value for exact equality and answers a
//! mismatch with Upgrade (F-563). Server code, gameplay-tag tables and paks
//! change between weekly builds too (F-686, F-700, F-702), so per-release
//! protocol metadata is data, not code: a [`ReleaseProfile`] is read from one
//! complete release file whose build id is only a label. `config/release.toml`
//! is the active release; frozen profiles of selected historical releases
//! live under `tests/fixtures/releases/` ([`ReleaseProfile::fixture`]), and
//! `tools/release_watch.py` checks, diffs and repins when Steam's public
//! branch moves; a re-pinned file is refused until its sections are
//! qualified ([`SECTIONS_QUALIFIED_FOR_BUILD_ID`]). Nothing here claims
//! compatibility with the pinned build.

use std::{collections::BTreeMap, fs, path::Path};

use anyhow::{Context, Result, bail, ensure};
use serde::Deserialize;

use super::{
    default_version_string_checksum,
    game_state_open::{ConnectedPlayersPlan, GameStateComponents, GameStateOpenPlan},
    manager_open::{
        ACTOR_STATE_CURRENT_BIOME, CONTAINER_INVENTORIES_HANDLE, ManagerComponent,
        ManagerInventories, ManagerOpenPlan, ManagerSlot, STAT_CONTAINER_HANDLE,
    },
    pawn_open::{PawnComponent, PawnContentPlan, PawnOpenPlan},
    quest_open::QuestOpenBasePlan,
    rep_layout::{
        ICARUS_ROW_HANDLE_NONE, ICARUS_ROW_HANDLE_TABLE_COUNT, ICARUS_TWO_BYTE_ROW_TABLES,
        ICARUS_ZERO_ROW_TABLES, MovementLevel, PawnLayoutVersion, RepValue, RotationLevel,
        TagEncoding, actor_component_rep_layout, actor_rep_layout, actor_state_rep_layout,
        bp_icarus_player_character_survival_rep_layout_for_version,
        bp_icarus_player_state_rep_layout, connected_players_rep_layout, flod_rep_layout,
        icarus_game_state_survival_rep_layout, icarus_player_controller_survival_rep_layout,
        inventory_container_manager_rep_layout, inventory_rep_layout, inventory_slot_commands,
        player_character_state_rep_layout, player_history_tracker_rep_layout,
        quest_manager_rep_layout, quest_rep_layout, radiation_manager_rep_layout,
        replicated_connected_player_rep_layout, weather_manager_component_rep_layout,
        world_talent_manager_rep_layout,
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
    /// Declared tag rows in `Data/Tags/D_Tags` (F-702); descriptive
    /// qualification count.
    pub data_table_tag_count: u32,
    /// Declared `GameplayTagList` entries in `DefaultGameplayTags.ini`
    /// (F-702); descriptive qualification count.
    pub ini_tag_count: u32,
    /// Unique sorted tag nodes after dot-prefix expansion (`NetIndexTrueBitNum`
    /// input, F-702).
    pub unique_node_count: u16,
    /// `NetIndexTrueBitNum` (`1..=16`, F-702).
    pub total_bits: u8,
    /// Configured `NetIndexFirstBitSegment` before clamping (`0..=16`, read
    /// from `DefaultGameplayTags.ini`, F-702); descriptive qualification metadata.
    pub configured_first_segment: u8,
    /// Effective `first_segment` passed to [`TagEncoding::NetIndex`] (`0` or
    /// `>= total_bits` selects unsplit `total_bits` packing, F-591, F-702).
    pub effective_first_segment: u8,
    /// `NumBitsForContainerSize` (`1..=8`, F-702); descriptive qualification
    /// metadata validated against `1..=8`.
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
/// Release-specific row widths applied to replication layouts. The standalone
/// low-level reader and writer keep their static registry
/// ([`Self::static_codec_registry`]).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseRowHandleProfile {
    /// Total static data-table names registered in `SetupTables`.
    pub table_count: u16,
    /// Sorted table indices whose packaged `DataTable` has 0 rows (0-byte row index).
    pub zero_row_tables: Vec<u16>,
    /// Sorted table indices whose packaged `DataTable` has 255..=65534 rows
    /// (2-byte row index).
    pub two_byte_row_tables: Vec<u16>,
}

impl ReleaseRowHandleProfile {
    /// The registry the standalone low-level `rep_layout` codec reads from its
    /// static `ICARUS_*_ROW_TABLES` tables. A profile whose `[row_handles]`
    /// equals it keeps the plain `RowHandle` commands; any other registry is
    /// applied as explicit widths ([`ReleaseProfile::apply_row_handle_profile`]).
    /// This is the codec's own table, not a fallback for an absent section.
    pub fn static_codec_registry() -> Self {
        Self {
            table_count: ICARUS_ROW_HANDLE_TABLE_COUNT,
            zero_row_tables: ICARUS_ZERO_ROW_TABLES.to_vec(),
            two_byte_row_tables: ICARUS_TWO_BYTE_ROW_TABLES.to_vec(),
        }
    }

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

    /// Row-index byte width (`0`, `1`, `2`) for `table_index` in this registry.
    ///
    /// [`ReleaseProfile::apply_row_handle_profile`] binds these widths into
    /// every layout a release plans, so they are wire-significant; only the
    /// plain `RowHandle` command of the standalone codec reads the static
    /// `ICARUS_*_ROW_TABLES` registry.
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
    pub inventory_container_manager: Vec<ReleasePathSegment>,
    pub player_history_tracker: Vec<ReleasePathSegment>,
    pub flod: Vec<ReleasePathSegment>,
    pub radiation_manager: Vec<ReleasePathSegment>,
    pub world_talent_manager: Vec<ReleasePathSegment>,
    pub disaster_controller: Vec<ReleasePathSegment>,
    /// `UIcarusStatContainer` and `UActorState` network checksums.
    pub stat_container_checksum: u32,
    pub actor_state_checksum: u32,
}

impl ReleaseManagersProfile {
    pub fn validate(&self) -> Result<()> {
        validate_archetype("managers quest_manager", &self.quest_manager)?;
        validate_archetype(
            "managers inventory_container_manager",
            &self.inventory_container_manager,
        )?;
        validate_archetype(
            "managers player_history_tracker",
            &self.player_history_tracker,
        )?;
        validate_archetype("managers flod", &self.flod)?;
        validate_archetype("managers radiation_manager", &self.radiation_manager)?;
        validate_archetype("managers world_talent_manager", &self.world_talent_manager)?;
        validate_archetype("managers disaster_controller", &self.disaster_controller)
    }
}

/// The FLOD manager's spawn location in M-707 (F-914).
const FLOD_SPAWN_LOCATION: [f32; 3] = [0.0, 0.0, -100_000.0];

/// Managers that open only with a save's state, in opening order.
const SAVED_MANAGER_SLOTS: [ManagerSlot; 4] = [
    ManagerSlot::Quest,
    ManagerSlot::InventoryContainer,
    ManagerSlot::PlayerHistory,
    ManagerSlot::Flod,
];

/// Per-release identifiers of the PlayerState open (`[player_state]`, F-784):
/// its archetype chain. Values come from the session and admission.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleasePlayerStateProfile {
    pub archetype: Vec<ReleasePathSegment>,
}

impl ReleasePlayerStateProfile {
    pub fn validate(&self) -> Result<()> {
        validate_archetype("player_state", &self.archetype)
    }
}

/// Per-release identities of level visibility actors (`[level_actors]`): the
/// object names of the static world-settings-class actors the original opens
/// in each level a client reports visible (field 82, F-1052), and the class
/// network checksum (`F-776`) their exports carry. Each actor is named by the
/// export chain `[name, PersistentLevel, world, package]`, the level part by
/// [`map_level_chain`] from the reported package. No packaged level content
/// is read: the names are authored constants of wire identities, uniform over
/// every level observed so far; a level whose actors are named otherwise
/// fails to resolve on the client.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReleaseLevelActorsProfile {
    /// Actor object names, opened in this order on fresh channels.
    pub names: Vec<String>,
    /// `ClassChecksum` of the actors' class.
    pub checksum: u32,
}

/// Most actors a level may name: each one takes an actor channel per
/// connection reporting the level.
pub const MAX_LEVEL_ACTOR_NAMES: usize = 4;

impl ReleaseLevelActorsProfile {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=MAX_LEVEL_ACTOR_NAMES).contains(&self.names.len()),
            "level_actors needs 1..={MAX_LEVEL_ACTOR_NAMES} actor names"
        );
        let mut seen = std::collections::BTreeSet::new();
        for name in &self.names {
            ensure!(
                !name.is_empty()
                    && name.is_ascii()
                    && !name.contains(['/', '.', ':'])
                    && name.len() <= 256,
                "level_actors names must be plain nonempty ASCII object names"
            );
            ensure!(
                seen.insert(name.to_ascii_lowercase()),
                "duplicate level_actors name {name}"
            );
        }
        ensure!(self.checksum != 0, "level_actors needs a class checksum");
        Ok(())
    }

    /// The export chain of actor `index` in the level `package`, leaf first:
    /// the actor (its class checksum), then `PersistentLevel`, the world and
    /// the package ([`map_level_chain`], checksum 0); every segment
    /// `no_load`, so each carries export flags 7 as the original's do.
    pub fn actor_chain(&self, index: usize, package: &str) -> Result<Vec<PathSegment>> {
        let name = self.names.get(index).context("level actor index")?;
        let mut chain = vec![PathSegment::new(name, self.checksum, true)];
        chain.extend(map_level_chain(package)?);
        Ok(chain)
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
    /// The class export chain of the `UReplicatedConnectedPlayer` records the
    /// `ConnectedPlayers` component lists: `ReplicatedConnectedPlayer` with its
    /// class network checksum, then `/Script/Icarus`. Optional; without it
    /// no record is sent.
    #[serde(default)]
    pub connected_player_record: Option<Vec<ReleasePathSegment>>,
}

impl ReleaseGameStateProfile {
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
        if let Some(chain) = &self.connected_player_record {
            validate_archetype("game_state connected_player_record", chain)?;
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
    /// Actor-property schema selected independently of exported class
    /// checksums; required, so a release never inherits a layout.
    pub layout_version: PawnLayoutVersion,
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

/// Data-driven protocol profile for one ICARUS release, read from one
/// complete release file (`docs/architecture.md#release-tracking`); the
/// library holds no per-release values.
///
/// `pin.network_checksum` is the Hello version
/// [`super::peer_lab::PeerLabConfig::with_release_profile`] sets.
/// `tags.tag_encoding()` and `class_net_cache` feed the layouts and field
/// bounds of the opens this profile plans and of `examples/capture_decode.rs`,
/// which also checks `engine_network_version`; `row_handles` binds the
/// row-handle widths of every layout this profile plans
/// ([`Self::apply_row_handle_profile`]). `game_network_version` and the
/// remaining `tags` fields record descriptive release-qualification metadata. `player_controller` feeds
/// [`super::session_open`]; `player_state`, `game_state`, `pawn` and
/// `managers` feed their open plans; `level_actors` names the actors a level
/// visibility report opens. Each is `None` exactly when the file omits its
/// section, which turns that open off ([`Self::parse`]).
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
    /// Level visibility actor identities; `None` where not derived, which
    /// leaves [`super::peer_lab::PeerLab::enable_level_visibility_actors`]
    /// unable to run. Identities alone enable nothing.
    pub level_actors: Option<ReleaseLevelActorsProfile>,
}

/// Key `tools/release_watch.py pin` writes into `[release]` when it re-pins a
/// file to a new build: the build the other sections were last qualified
/// for. While it is present the file is no profile ([`ReleaseProfile::parse`]
/// refuses it); [`ReleasePin::parse`] still reads its identity.
pub const SECTIONS_QUALIFIED_FOR_BUILD_ID: &str = "sections_qualified_for_build_id";

/// The `[release]` table as written: the pin plus the carry-over key.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseTable {
    app_id: u32,
    build_id: u64,
    project_version: String,
    network_checksum: u32,
    #[serde(default)]
    sections_qualified_for_build_id: Option<u64>,
}

impl ReleaseTable {
    /// The pin, refusing a table whose other sections belong to another build.
    fn qualified_pin(self) -> Result<ReleasePin> {
        if let Some(qualified) = self.sections_qualified_for_build_id {
            bail!(
                "release file pins build {} ({}) but its other sections were last \
                 qualified for build {qualified} ({SECTIONS_QUALIFIED_FOR_BUILD_ID}); \
                 qualify every remaining section for build {}, then remove \
                 {SECTIONS_QUALIFIED_FOR_BUILD_ID} from [release]",
                self.build_id,
                self.project_version,
                self.build_id
            );
        }
        Ok(self.pin())
    }

    fn pin(self) -> ReleasePin {
        ReleasePin {
            app_id: self.app_id,
            build_id: self.build_id,
            project_version: self.project_version,
            network_checksum: self.network_checksum,
        }
    }
}

/// The `[release]` table of a release file; the other sections are the
/// profile's and are ignored here.
#[derive(Deserialize)]
struct ReleasePinFile {
    release: ReleaseTable,
}

/// A release file as written. Required sections are `Option` here only so
/// [`ReleaseProfile::parse`] can name a missing one.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReleaseFile {
    release: ReleaseTable,
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
    #[serde(default)]
    level_actors: Option<ReleaseLevelActorsProfile>,
}

impl ReleasePin {
    /// Parse the `[release]` table of a release file and check its stored
    /// checksum. A re-pinned file whose other sections are still marked with
    /// [`SECTIONS_QUALIFIED_FOR_BUILD_ID`] is read too: this is its identity,
    /// not a profile.
    pub fn parse(text: &str) -> Result<Self> {
        let parsed = toml::from_str::<ReleasePinFile>(text).context("release pin")?;
        let pin = parsed.release.pin();
        pin.validate()?;
        Ok(pin)
    }

    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path).with_context(|| path.display().to_string())?)
    }

    pub(crate) fn validate(&self) -> Result<()> {
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
        if let Some(level_actors) = &self.level_actors {
            level_actors.validate()?;
        }
        Ok(())
    }

    /// Every package path (a chain segment starting with `/`) this release's
    /// own opens may export: the archetype and class chains of the
    /// PlayerController, PlayerState, GameState (with its game mode,
    /// spectator and record classes), pawn (with its inventory class) and
    /// managers. Known before any client connects; the map's level chain is
    /// the caller's.
    pub fn server_package_paths(&self) -> Vec<String> {
        let mut chains: Vec<&[ReleasePathSegment]> = Vec::new();
        if let Some(controller) = &self.player_controller {
            chains.push(&controller.archetype);
        }
        if let Some(player_state) = &self.player_state {
            chains.push(&player_state.archetype);
        }
        if let Some(game_state) = &self.game_state {
            chains.extend([
                game_state.archetype.as_slice(),
                game_state.game_mode_class.as_slice(),
                game_state.spectator_class.as_slice(),
            ]);
            if let Some(record) = &game_state.connected_player_record {
                chains.push(record);
            }
        }
        if let Some(pawn) = &self.pawn {
            chains.extend([pawn.archetype.as_slice(), pawn.inventory_class.as_slice()]);
        }
        if let Some(managers) = &self.managers {
            chains.extend([
                managers.quest_manager.as_slice(),
                managers.inventory_container_manager.as_slice(),
                managers.player_history_tracker.as_slice(),
                managers.flod.as_slice(),
                managers.radiation_manager.as_slice(),
                managers.world_talent_manager.as_slice(),
                managers.disaster_controller.as_slice(),
            ]);
        }
        chains
            .into_iter()
            .flatten()
            .filter(|segment| segment.path.starts_with('/'))
            .map(|segment| segment.path.clone())
            .collect()
    }

    /// Parse a complete release TOML file and validate it.
    ///
    /// The build id is only a label: no value is looked up by it, and an
    /// absent section never takes another release's values. `[release]`,
    /// `[tags]`, `[class_net_cache]` and `[row_handles]` are required: the
    /// Hello version derives from `[release]`, and every open the profile
    /// enables takes its tag encoding, `FClassNetCache` field bounds and
    /// row-handle widths from the other three; a file without one fails with
    /// an error naming it (pin-only files are no longer accepted).
    /// `[player_controller]`, `[player_state]`, `[game_state]`, `[pawn]`,
    /// `[managers]` and `[level_actors]` are optional: absent means the
    /// release does not carry that open, which stays off.
    ///
    /// A `[release]` table carrying [`SECTIONS_QUALIFIED_FOR_BUILD_ID`] was
    /// re-pinned to a new build while its other sections remain the earlier
    /// build's; it is refused with an error naming both builds, never served.
    pub fn parse(text: &str) -> Result<Self> {
        let parsed = toml::from_str::<ReleaseFile>(text).context("release profile")?;
        fn required<T>(section: &str, value: Option<T>) -> Result<T> {
            value.with_context(|| {
                format!(
                    "release profile lacks required section [{section}]; pin-only release \
                     files are no longer accepted: use a complete profile (examples: \
                     tests/fixtures/releases/)"
                )
            })
        }
        let pin = parsed.release.qualified_pin()?;
        let profile = Self {
            tags: required("tags", parsed.tags)?,
            class_net_cache: required("class_net_cache", parsed.class_net_cache)?,
            row_handles: required("row_handles", parsed.row_handles)?,
            pin,
            engine_network_version: ENGINE_NETWORK_VERSION,
            game_network_version: GAME_NETWORK_VERSION,
            player_controller: parsed.player_controller,
            player_state: parsed.player_state,
            game_state: parsed.game_state,
            pawn: parsed.pawn,
            managers: parsed.managers,
            level_actors: parsed.level_actors,
        };
        profile.validate()?;
        Ok(profile)
    }

    /// Apply this release's row-handle registry to a layout, including nested arrays.
    pub fn apply_row_handle_profile(
        &self,
        layout: &mut [super::rep_layout::RepCommand],
    ) -> Result<()> {
        self.row_handles.validate()?;
        if self.row_handles == ReleaseRowHandleProfile::static_codec_registry() {
            fn restore_legacy(layout: &mut [super::rep_layout::RepCommand]) {
                use super::rep_layout::RepCommand;
                for command in layout {
                    match command {
                        RepCommand::RowHandleWithWidths { .. } => *command = RepCommand::RowHandle,
                        RepCommand::DynamicArray { element } => restore_legacy(element),
                        _ => {}
                    }
                }
            }
            restore_legacy(layout);
            return Ok(());
        }
        let widths = (0..self.row_handles.table_count)
            .map(|index| self.row_handles.row_width_bytes(index))
            .collect::<Result<Vec<_>>>()?;
        super::rep_layout::apply_row_handle_widths(layout, &widths)
    }

    fn row_layout(
        &self,
        mut layout: Vec<super::rep_layout::RepCommand>,
    ) -> Result<Vec<super::rep_layout::RepCommand>> {
        self.apply_row_handle_profile(&mut layout)?;
        Ok(layout)
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
            actor_layout: self.row_layout(icarus_player_controller_survival_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
                self.tags.tag_encoding(),
            ))?,
            component_layout: self.row_layout(actor_component_rep_layout())?,
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
            layout: self.row_layout(bp_icarus_player_state_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
            ))?,
            engine_net_version: self.engine_network_version,
            channel_index: PLAYER_STATE_CHANNEL_INDEX,
        }))
    }

    /// The world manager open plans for the map package of `map_url`, or
    /// none without `[managers]` (F-859): the radiation and world-talent
    /// managers carry a stat container and an actor state (`CurrentBiome`
    /// row 0, as captured), the disaster controller a stat container only.
    /// The managers that open only with a save's state are planned by
    /// [`Self::saved_manager_open_plans`].
    pub fn manager_open_plans(&self, map_url: &str) -> Result<Vec<(ManagerSlot, ManagerOpenPlan)>> {
        Ok(self
            .all_manager_open_plans(map_url)?
            .into_iter()
            .filter(|(slot, _)| !SAVED_MANAGER_SLOTS.contains(slot))
            .collect())
    }

    /// The open plans of the managers that replicate a save's state, or
    /// none without `[managers]`: the QuestManager (F-861), the
    /// InventoryContainerManager (F-866) and the PlayerHistoryTracker
    /// (F-862), and the FLOD manager (F-914), without saved state, which
    /// the `ManagerOpenPlan::with_*` methods add.
    pub fn saved_manager_open_plans(
        &self,
        map_url: &str,
    ) -> Result<Vec<(ManagerSlot, ManagerOpenPlan)>> {
        Ok(self
            .all_manager_open_plans(map_url)?
            .into_iter()
            .filter(|(slot, _)| SAVED_MANAGER_SLOTS.contains(slot))
            .collect())
    }

    /// The open plan of the manager in `slot` for the map package of
    /// `map_url`, or `None` without `[managers]`.
    pub fn manager_open_plan(
        &self,
        slot: ManagerSlot,
        map_url: &str,
    ) -> Result<Option<ManagerOpenPlan>> {
        Ok(self
            .all_manager_open_plans(map_url)?
            .into_iter()
            .find_map(|(candidate, plan)| (candidate == slot).then_some(plan)))
    }

    /// The base `AQuest` open plan (`C019`, `F-934`, `F-935`) for the map
    /// package of `map_url`, or `None` without `[managers]` or when
    /// `pin.build_id` is outside the builds whose `QUEST_CLASSES` checksums
    /// and `Check()` rules are qualified (`25793607`/`25793767` for 3.0.31,
    /// `25956137` for 3.0.32).
    pub fn quest_open_base_plan(&self, map_url: &str) -> Result<Option<QuestOpenBasePlan>> {
        let Some(managers) = &self.managers else {
            return Ok(None);
        };
        if !matches!(self.pin.build_id, 25_793_607 | 25_793_767 | 25_956_137) {
            return Ok(None);
        }
        Ok(Some(QuestOpenBasePlan {
            level: map_level_chain(map_url)?,
            layout: self.row_layout(quest_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
                self.tags.tag_encoding(),
            ))?,
            actor_state_layout: self.row_layout(actor_state_rep_layout())?,
            stat_container_checksum: managers.stat_container_checksum,
            actor_state_checksum: managers.actor_state_checksum,
            engine_net_version: self.engine_network_version,
        }))
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
        let icarus_components = || -> Result<Vec<ManagerComponent>> {
            Ok(vec![
                ManagerComponent::stat_container(
                    managers.stat_container_checksum,
                    Some(STAT_CONTAINER_HANDLE),
                ),
                ManagerComponent::actor_state(
                    managers.actor_state_checksum,
                    self.row_layout(actor_state_rep_layout())?,
                    BTreeMap::from([(ACTOR_STATE_CURRENT_BIOME, RepValue::Byte(0))]),
                ),
            ])
        };
        let plan = |archetype: &[ReleasePathSegment], layout, components| ManagerOpenPlan {
            archetype: session_segments(archetype),
            level: level.clone(),
            location: None,
            layout,
            components,
            values: BTreeMap::new(),
            actors: BTreeMap::new(),
            keyed_actors: BTreeMap::new(),
            actor_arrays: BTreeMap::new(),
            inventories: None,
            engine_net_version: self.engine_network_version,
        };
        let mut plans = vec![
            (
                ManagerSlot::Quest,
                plan(
                    &managers.quest_manager,
                    self.row_layout(quest_manager_rep_layout(location, velocity, rotation, tags))?,
                    icarus_components()?,
                ),
            ),
            (
                ManagerSlot::WorldTalent,
                plan(
                    &managers.world_talent_manager,
                    self.row_layout(world_talent_manager_rep_layout(
                        location, velocity, rotation, tags,
                    ))?,
                    icarus_components()?,
                ),
            ),
            (
                ManagerSlot::Radiation,
                plan(
                    &managers.radiation_manager,
                    self.row_layout(radiation_manager_rep_layout(
                        location, velocity, rotation, tags,
                    ))?,
                    icarus_components()?,
                ),
            ),
            (
                ManagerSlot::PlayerHistory,
                plan(
                    &managers.player_history_tracker,
                    self.row_layout(player_history_tracker_rep_layout(
                        location, velocity, rotation, tags,
                    ))?,
                    icarus_components()?,
                ),
            ),
            (
                ManagerSlot::Flod,
                ManagerOpenPlan {
                    // As captured (F-914); where the original takes it from
                    // is unread.
                    location: Some(FLOD_SPAWN_LOCATION),
                    ..plan(
                        &managers.flod,
                        self.row_layout(flod_rep_layout(location, velocity, rotation))?,
                        Vec::new(),
                    )
                },
            ),
            (
                ManagerSlot::Disaster,
                plan(
                    &managers.disaster_controller,
                    self.row_layout(actor_rep_layout(location, velocity, rotation))?,
                    vec![ManagerComponent::stat_container(
                        managers.stat_container_checksum,
                        None,
                    )],
                ),
            ),
        ];
        // The InventoryContainerManager's inventories are the pawn's
        // runtime inventories, without the pawn's owning component, so it
        // has a plan only with `[pawn]`.
        if let Some(pawn) = &self.pawn {
            let container = ManagerOpenPlan {
                inventories: Some(ManagerInventories {
                    handle: CONTAINER_INVENTORIES_HANDLE,
                    blocks: self.pawn_content_plan(pawn)?,
                    inventories: Vec::new(),
                }),
                ..plan(
                    &managers.inventory_container_manager,
                    self.row_layout(inventory_container_manager_rep_layout(
                        location, velocity, rotation, tags,
                    ))?,
                    Vec::new(),
                )
            };
            let at = plans
                .iter()
                .position(|(slot, _)| *slot == ManagerSlot::PlayerHistory)
                .unwrap_or(plans.len());
            plans.insert(at, (ManagerSlot::InventoryContainer, container));
        }
        Ok(plans)
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
            layout: self.row_layout(icarus_game_state_survival_rep_layout(
                MovementLevel::Whole,
                MovementLevel::Whole,
                RotationLevel::Bytes,
            ))?,
            weather_layout: self.row_layout(weather_manager_component_rep_layout())?,
            engine_net_version: self.engine_network_version,
        }))
    }

    /// The GameState `ConnectedPlayers` record plan: the record class chain
    /// and the `UReplicatedConnectedPlayer` and `UConnectedPlayers` layouts,
    /// or `None` without `[game_state]` or its `connected_player_record`.
    pub fn connected_players_plan(&self) -> Result<Option<ConnectedPlayersPlan>> {
        let Some(chain) = self
            .game_state
            .as_ref()
            .and_then(|game_state| game_state.connected_player_record.as_ref())
        else {
            return Ok(None);
        };
        Ok(Some(ConnectedPlayersPlan {
            record_class: session_segments(chain),
            record_layout: self.row_layout(replicated_connected_player_rep_layout())?,
            component_layout: self.row_layout(connected_players_rep_layout())?,
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
            layout: self.row_layout(bp_icarus_player_character_survival_rep_layout_for_version(
                MovementLevel::Hundredths,
                MovementLevel::Whole,
                RotationLevel::Bytes,
                self.tags.tag_encoding(),
                pawn.layout_version,
            ))?,
            layout_version: pawn.layout_version,
            content: self.pawn_content_plan(pawn)?,
            engine_net_version: self.engine_network_version,
        }))
    }

    /// The pawn's content-block identifiers and layouts, which the
    /// InventoryContainerManager's inventories share (F-866).
    fn pawn_content_plan(&self, pawn: &ReleasePawnProfile) -> Result<PawnContentPlan> {
        Ok(PawnContentPlan {
            component_layout: self.row_layout(actor_component_rep_layout())?,
            character_state_layout: self.row_layout(player_character_state_rep_layout())?,
            inventory_class: session_segments(&pawn.inventory_class),
            inventory_layout: self.row_layout(inventory_rep_layout())?,
            slot_layout: self.row_layout(inventory_slot_commands(self.tags.tag_encoding()))?,
            stat_container_bound: self.class_net_cache.stat_container,
            inventory_bound: self.class_net_cache.inventory,
        })
    }

    /// Load and validate a release profile from disk.
    pub fn load(path: &Path) -> Result<Self> {
        Self::parse(&fs::read_to_string(path).with_context(|| path.display().to_string())?)
    }
}

/// Directory of frozen release profiles: complete files of selected
/// historical builds, kept for tests that need that release's layout and
/// named by project version prefix (for example `3.0.32.toml`). Not every
/// release gets one. They are test and research inputs, not server
/// configuration: the server loads only the file its configuration names.
pub const RELEASE_FIXTURE_DIR: &str =
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/releases");

impl ReleaseProfile {
    /// Path of the frozen release profile `name` (`"3.0.32"` names
    /// `tests/fixtures/releases/3.0.32.toml`).
    pub fn fixture_path(name: &str) -> std::path::PathBuf {
        assert!(
            !name.is_empty() && !name.contains(['/', '\\']) && name != "..",
            "release fixture names are plain file stems"
        );
        Path::new(RELEASE_FIXTURE_DIR).join(format!("{name}.toml"))
    }

    /// Load the frozen release profile `name` for tests and examples; panics,
    /// naming the file, when it is missing or invalid.
    pub fn fixture(name: &str) -> Self {
        let path = Self::fixture_path(name);
        Self::load(&path)
            .unwrap_or_else(|error| panic!("release fixture {}: {error:#}", path.display()))
    }
}
