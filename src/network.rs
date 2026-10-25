//! EXPERIMENTAL opt-in session layer for `icarus-server run`.
//!
//! When `[network] enabled = true`, the production run binds one IPv4
//! loopback UDP socket before any world state is opened and installs the
//! loopback peer lab ([`PeerLab`] + [`PeerLabPlugin`]) with game admission
//! enabled: handshake, Hello/Challenge, Login/Welcome and Join against the
//! inferred admission model, nothing more. No replication, gameplay or
//! identity verification runs, and nothing here demonstrates compatibility
//! with any ICARUS client. Status records carry counts only.
//!
//! With `[network] release_profile` set, the release TOML is loaded and
//! validated before the socket binds and its network checksum becomes the
//! lab's local Hello version: a differing Hello gets Upgrade and the peer is
//! removed (`upgrades_sent`, `closed_by_reply`). Unset, any version is accepted.
//! When the profile carries `[player_controller]` identifiers (built in for
//! the qualified builds, F-780), every successful Join is answered with the
//! PlayerController open on actor channel 2 (F-778, F-779): exports, spawn at
//! [`SPAWN_LOCATION`], roles, references and component blocks; with `[pawn]`
//! identifiers the player's pawn follows on channel 3 (F-797: captured roles
//! and references, component bools, [`pawn_content`]: with the official
//! data tables a fresh player's vitals and empty inventories, F-836; no
//! stats);
//! with `[game_state]` identifiers the world's GameState on channel 4 (F-794:
//! [`game_state_values`], plus a preserved world's prospect and game mode
//! record, [`saved_game_state`], whose `TimeOfDay` then advances at the
//! constant rate of its saved `SecondsPerGameDay`, F-806); with
//! `[player_state]` identifiers the referenced PlayerState on channel 5
//! (F-784: PlayerId from 256, the Login name and unique net id, `bIsHost`
//! for a player joining with no other joined player, F-913). Each open takes as many datagrams as it
//! needs. Once a connection acks the GameState open, its world time, level
//! clock, time of day and `ServerFPS` (this server's frame rate over its
//! poll intervals) follow as property updates (F-805), and with a loaded
//! world and the official data tables its `LockedMissions` (F-834); with
//! [`apply_server_settings`], `TimeBetweenSaves` and a locally saving
//! server's `NetworkingStatus` listing the joined players (F-810). Nothing
//! else replicates.
//!
//! Ordering: the lab polls at most `PACKETS_PER_UPDATE` datagrams once per
//! app update in `RunFixedMainLoopSystems::BeforeFixedMainLoop`, so network
//! input is processed before every `SimulationSet` of that update's fixed
//! ticks. Shutdown first stops admission ([`stop_admission`]), then sends
//! every session the original's shutdown notice (Failure "Host closed the
//! connection.", then the control channel close; F-782), removes them all and
//! drops the socket ([`teardown`]); both are idempotent.
use std::net::{SocketAddr, SocketAddrV4};

use std::collections::BTreeMap;

use anyhow::{Context, Result, bail, ensure};
use bevy::prelude::{App, Resource, World};

use crate::{
    config::{NetworkConfig, read_configuration},
    data_pak::DataTables,
    experimental::{
        admission::AdmissionPolicy,
        control_messages::ArchiveText,
        game_state_open::{
            GAME_STATE_CHANNEL_INDEX, GameStateManagers, GameStateOpenState, LockedMission,
            MATCH_STATE_IN_PROGRESS, NetworkingStatus, active_prospect_values, handle,
            saved_game_mode_values, session_flags_value,
        },
        inventory_weight::{
            AUTHORITY_LOCAL_ROLE, DEFAULT_CDO_CURRENT_WEIGHT, DYNAMIC_PROP_ASSOCIATED_INVENTORY_ID,
            DYNAMIC_PROP_FILLABLE_UNITS, DYNAMIC_PROP_ITEMABLE_STACK, ItemWeightInput,
            RestoredContainerInventory, recalculate_inventory_weight,
        },
        manager_open::{ManagerOpenPlan, ManagerSlot},
        pawn_open::{
            CharacterVitals, FastArrayContent, InventorySlot, PAWN_CHANNEL_INDEX, PawnContent,
            PawnInventory, SlotItem,
        },
        peer_lab::{PeerLab, PeerLabAdmission, PeerLabConfig, PeerLabPlugin, SessionCounts},
        prospect_info::{
            MissionHistoryRecord, PlayerHistoryEntry, SavedContainerManager, SavedItem,
            SavedQuestManager,
        },
        release::{ReleasePin, ReleaseProfile},
        rep_layout::{DYNAMIC_ITEM_PROPERTIES_MAX_VALUE, NetworkName, RepValue},
        server_fps::{DEFAULT_SAMPLES, ServerFpsMeter},
        session_open::ActorKey,
        time_of_day::{RateCurve, TimeOfDayClock},
    },
    preservation::PreservedSection,
};

/// The free official ICARUS dedicated server's Steam app (F-828).
pub const OFFICIAL_SERVER_APP_ID: u32 = 2089300;
/// Where `install-official-data` puts the official server's files, below
/// the data directory.
pub const OFFICIAL_DIR: &str = "official";
/// The official server's data tables (F-829).
pub const OFFICIAL_DATA_PAK: &str = "Icarus/Content/Data/data.pak";

/// Write `record` as `install.json` in `official` durably: a synced
/// temporary file renamed into place, then the directory synced, so a
/// crash leaves the old record or the new one, never a partial file.
pub fn write_install_record(official: &std::path::Path, record: &serde_json::Value) -> Result<()> {
    use std::io::Write;
    let temporary = official.join("install.json.tmp");
    let mut file = std::fs::File::create(&temporary).context("create install.json.tmp")?;
    file.write_all(&serde_json::to_vec_pretty(record)?)?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(&temporary, official.join("install.json")).context("publish install.json")?;
    std::fs::File::open(official)?.sync_all()?;
    Ok(())
}

/// The data table whose row order `SessionFlags` indices follow.
pub const SESSION_FLAGS_TABLE: &str = "D_SessionFlags";
/// The data table whose row order `LockedMissions` rows follow (F-834).
pub const FACTION_MISSIONS_TABLE: &str = "D_FactionMissions";
/// The `D_FactionMissions` field a row clears to never lock (F-834); a row
/// that omits it keeps the struct default, set.
pub const RETRY_TIMEOUT_FIELD: &str = "bUseOpenWorldRetryTimeout";

/// Spectator slots granted beside `max_players` (the lab's default policy).
pub const SPECTATOR_SLOTS: u32 = 2;
/// Lab choice: the PlayerController spawn location (no world geometry or
/// PlayerStart is consulted).
pub const SPAWN_LOCATION: [f32; 3] = [0.0, 0.0, 0.0];

/// The first channel of the world manager opens, after the PlayerState's.
pub const MANAGER_FIRST_CHANNEL: u32 = 6;

/// The session layer's key for the world's one GameState.
pub const GAME_STATE_KEY: ActorKey = ActorKey::Session(0);

/// The GameState values `run` opens every connection with: play has begun,
/// the match is in progress (as captured, F-794) and this run's
/// `RichPresenceGUID` (F-912); the lab adds the world time, and a loaded
/// save its state ([`saved_game_state`]).
pub fn game_state_values() -> Result<GameStateOpenState> {
    Ok(GameStateOpenState {
        key: GAME_STATE_KEY,
        managers: GameStateManagers::default(),
        values: BTreeMap::from([
            (handle::HAS_BEGUN_PLAY, RepValue::Bool(true)),
            (
                handle::MATCH_STATE,
                RepValue::NetworkName(NetworkName::String {
                    text: ArchiveText::text(MATCH_STATE_IN_PROGRESS)?,
                    number: 0,
                }),
            ),
            (
                handle::RICH_PRESENCE_GUID,
                RepValue::Str(ArchiveText::text(&rich_presence_guid()?)?),
            ),
        ]),
        weather: Vec::new(),
    })
}

/// A fresh `RichPresenceGUID` (F-912): the original's GameState sets it at
/// `BeginPlay` to `FGuid::NewGuid().ToString(EGuidFormats::Digits)`, 32
/// uppercase hex digits, once per server run.
pub fn rich_presence_guid() -> Result<String> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(|error| anyhow::anyhow!("system randomness: {error}"))?;
    Ok(bytes.iter().map(|byte| format!("{byte:02X}")).collect())
}

/// The lab vitals a pawn opens with when no data tables are loaded; they
/// are not ICARUS balance values.
const LAB_VITALS: CharacterVitals = CharacterVitals {
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

/// The pawn content `run` opens every player's pawn with: movement frozen
/// at open (as captured, F-797). With the official server's data tables a
/// fresh player's vitals, empty inventories and replicated stats come from
/// them ([`fresh_player_vitals`], [`player_inventories`], F-836;
/// [`crate::pawn_stats::fresh_player_stats`], F-839); without, lab vitals,
/// no inventories and no stats. No `IcarusUID` or tags are sent: they need
/// imported or catalogued content.
pub fn pawn_content(tables: Option<&mut DataTables>) -> Result<PawnContent> {
    let (vitals, inventories, stats) = match tables {
        Some(tables) => (
            fresh_player_vitals(tables)?,
            player_inventories(tables)?,
            Some(crate::pawn_stats::fresh_player_stats(
                tables,
                STARTING_STATS_TABLE,
                STARTING_STATS_ROW,
            )?),
        ),
        None => (LAB_VITALS, Vec::new(), None),
    };
    Ok(PawnContent {
        frozen_movement: true,
        character_state: vitals.values(),
        stats,
        inventories,
        ..PawnContent::default()
    })
}

/// The data tables a fresh player's pawn content comes from (F-836).
pub const INVENTORY_TABLE: &str = "D_Inventory";
pub const INVENTORY_INFO_TABLE: &str = "D_InventoryInfo";
pub const TAG_QUERIES_TABLE: &str = "D_TagQueries";
pub const ITEMS_STATIC_TABLE: &str = "D_ItemsStatic";
/// The item trait table whose `Weight` feeds ticked container `CurrentWeight`
/// (`U-075`).
pub const ITEMABLE_TABLE: &str = "D_Itemable";
pub const STARTING_STATS_TABLE: &str = "D_CharacterStartingStats";
pub const CHARACTER_GROWTH_TABLE: &str = "D_CharacterGrowth";
/// The player's rows in `D_Inventory`, `D_CharacterStartingStats` and
/// `D_CharacterGrowth`.
pub const PLAYER_INVENTORY_ROW: &str = "Player";
pub const STARTING_STATS_ROW: &str = "Base_Stats";
pub const PLAYER_GROWTH_ROW: &str = "Player";
/// The slot arrays' replication key at open, as captured (F-799).
pub const INVENTORY_SLOTS_REPLICATION_KEY: i32 = 2;
/// The `D_TagQueries` row `UInventoryItemLibrary::GetItemWeight` evaluates
/// before adding dynamic fillable units (`10`) (`U-075`).
const WEIGHABLE_RESOURCE_CONTAINER_QUERY: &str = "Any_Weighable_Resource_Container";

/// The row named by a `{"RowName": ...}` handle.
fn row_name(value: &serde_json::Value) -> Option<&str> {
    value.get("RowName")?.as_str()
}

/// Tags listed in `D_TagQueries`'s `Any_Weighable_Resource_Container` when
/// its `QueryTokenStream` is an `AnyTagsMatch` (`[0, 1, 1, N, 0..N]`).
fn weighable_resource_tags(tables: &mut DataTables) -> Result<Vec<String>> {
    let row = tables
        .row(TAG_QUERIES_TABLE, WEIGHABLE_RESOURCE_CONTAINER_QUERY)?
        .with_context(|| {
            format!("{TAG_QUERIES_TABLE} lacks {WEIGHABLE_RESOURCE_CONTAINER_QUERY}")
        })?;
    let query = row
        .get("Query")
        .and_then(serde_json::Value::as_object)
        .with_context(|| format!("{WEIGHABLE_RESOURCE_CONTAINER_QUERY} has no Query object"))?;
    let tags: Vec<String> = query
        .get("TagDictionary")
        .and_then(serde_json::Value::as_array)
        .with_context(|| format!("{WEIGHABLE_RESOURCE_CONTAINER_QUERY} has no TagDictionary"))?
        .iter()
        .map(|entry| {
            entry
                .get("TagName")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned)
                .with_context(|| {
                    format!(
                        "{WEIGHABLE_RESOURCE_CONTAINER_QUERY} TagDictionary entry has no TagName"
                    )
                })
        })
        .collect::<Result<_>>()?;
    let stream: Vec<i64> = query
        .get("QueryTokenStream")
        .and_then(serde_json::Value::as_array)
        .with_context(|| format!("{WEIGHABLE_RESOURCE_CONTAINER_QUERY} has no QueryTokenStream"))?
        .iter()
        .map(|token| {
            token.as_i64().with_context(|| {
                format!("{WEIGHABLE_RESOURCE_CONTAINER_QUERY} has a non-integer token")
            })
        })
        .collect::<Result<_>>()?;
    let count = i64::try_from(tags.len())?;
    let expected: Vec<i64> = [0, 1, 1, count].into_iter().chain(0..count).collect();
    ensure!(
        stream == expected,
        "{WEIGHABLE_RESOURCE_CONTAINER_QUERY} uses an unsupported QueryTokenStream"
    );
    Ok(tags)
}

fn item_matches_any_tag(
    static_row: &serde_json::Map<String, serde_json::Value>,
    query_tags: &[String],
) -> Result<bool> {
    for container_key in ["Manual_Tags", "Generated_Tags"] {
        let Some(container) = static_row.get(container_key) else {
            continue;
        };
        let container = container
            .as_object()
            .with_context(|| format!("an item's {container_key} is not an object"))?;
        for array_key in ["GameplayTags", "ParentTags"] {
            let Some(entries) = container.get(array_key) else {
                continue;
            };
            let entries = entries.as_array().with_context(|| {
                format!("an item's {container_key}.{array_key} is not an array")
            })?;
            for entry in entries {
                let tag = entry
                    .get("TagName")
                    .and_then(serde_json::Value::as_str)
                    .with_context(|| {
                        format!("an item's {container_key}.{array_key} entry has no TagName")
                    })?;
                if query_tags.iter().any(|target| {
                    tag == target
                        || tag
                            .strip_prefix(target.as_str())
                            .is_some_and(|rest| rest.starts_with('.'))
                }) {
                    return Ok(true);
                }
            }
        }
    }
    Ok(false)
}

fn ticked_item_weight_input(
    tables: &mut DataTables,
    item: &SavedItem,
    weighable_tags: &[String],
) -> Result<ItemWeightInput> {
    ensure!(
        item.stats.is_empty(),
        "a ticked container item carries custom stats"
    );
    ensure!(
        !item.dynamic.iter().any(|&(property, value)| {
            property == i32::from(DYNAMIC_PROP_ASSOCIATED_INVENTORY_ID) && value >= 0
        }),
        "a ticked container item carries a nested inventory"
    );
    let item_name = String::from_utf16(&item.row_name).context("saved item row name")?;
    let static_row = tables
        .row(ITEMS_STATIC_TABLE, &item_name)?
        .with_context(|| format!("{ITEMS_STATIC_TABLE} lacks a saved item"))?;
    if let Some(additional) = static_row.get("AdditionalStats") {
        let map = additional
            .as_object()
            .context("an item's AdditionalStats is not a map")?;
        if map.keys().any(|key| key.contains("Weight_+")) {
            bail!("a ticked container item's static row grants a weight stat");
        }
    }
    let Some(itemable_name) = static_row
        .get("Itemable")
        .and_then(row_name)
        .filter(|name| *name != "None")
    else {
        return Ok(ItemWeightInput {
            has_valid_itemable: false,
            weight_stat: 0,
            matches_weighable_resource_container: false,
            fillable_units: None,
            itemable_stack: None,
            matching_carry_weight_stat_pct: 0,
        });
    };
    let itemable_row = tables
        .row(ITEMABLE_TABLE, itemable_name)?
        .with_context(|| format!("{ITEMABLE_TABLE} lacks an itemable row"))?;
    let weight_stat = match itemable_row.get("Weight") {
        None => 0,
        Some(value) => value
            .as_i64()
            .and_then(|weight| i32::try_from(weight).ok())
            .context("an itemable Weight is out of range")?,
    };
    let matches_weighable_resource_container = item_matches_any_tag(&static_row, weighable_tags)?;
    let dynamic_value = |target: u8| {
        item.dynamic
            .iter()
            .find(|&&(property, _)| property == i32::from(target))
            .map(|&(_, value)| value)
    };
    Ok(ItemWeightInput {
        has_valid_itemable: true,
        weight_stat,
        matches_weighable_resource_container,
        fillable_units: dynamic_value(DYNAMIC_PROP_FILLABLE_UNITS),
        itemable_stack: dynamic_value(DYNAMIC_PROP_ITEMABLE_STACK),
        matching_carry_weight_stat_pct: 0,
    })
}

/// A fresh player's inventories (F-836), in the order `D_Inventory`'s
/// `Player` row lists them: each one's `D_InventoryInfo` row and its
/// `StartingSlots` empty slots (none when omitted), slot `i` querying the
/// `SlotOverrides` entry at `Location` `i`, else the `SlotTemplate`, as a
/// `D_TagQueries` row.
pub fn player_inventories(tables: &mut DataTables) -> Result<Vec<PawnInventory>> {
    let player = tables
        .row(INVENTORY_TABLE, PLAYER_INVENTORY_ROW)?
        .with_context(|| format!("{INVENTORY_TABLE} has no {PLAYER_INVENTORY_ROW} row"))?;
    let listed = player
        .get("Inventories")
        .and_then(serde_json::Value::as_array)
        .context("the player's Inventories is not an array")?;
    listed
        .iter()
        .map(|handle| {
            let name = row_name(handle).context("an inventory names no row")?;
            info_inventory(tables, &name.encode_utf16().collect::<Vec<u16>>())
        })
        .collect()
}

/// The inventory a `D_InventoryInfo` row describes, empty: its row and its
/// `StartingSlots` empty slots (none when omitted), slot `i` querying the
/// `SlotOverrides` entry at `Location` `i`, else the `SlotTemplate`, as a
/// `D_TagQueries` row (F-836).
fn info_inventory(tables: &mut DataTables, name: &[u16]) -> Result<PawnInventory> {
    let index = tables
        .row_index(INVENTORY_INFO_TABLE, name)?
        .with_context(|| format!("{INVENTORY_INFO_TABLE} lacks an inventory"))?;
    let info = tables
        .row(INVENTORY_INFO_TABLE, &String::from_utf16(name)?)?
        .context("an inventory info row vanished")?;
    let starting = match info.get("StartingSlots") {
        None => 0,
        Some(value) => value
            .as_i64()
            .and_then(|slots| i32::try_from(slots).ok())
            .filter(|slots| (0..=MAX_STARTING_SLOTS).contains(slots))
            .context("an inventory's StartingSlots is out of range")?,
    };
    let template = info.get("SlotTemplate").and_then(row_name);
    // A malformed override fails, as a changed table shape would
    // otherwise send the template's query for its slot.
    let overrides: Vec<(i64, &str)> = match info.get("SlotOverrides") {
        None => Vec::new(),
        Some(entries) => entries
            .as_array()
            .context("an inventory's SlotOverrides is not an array")?
            .iter()
            .map(|entry| {
                let location = entry.get("Location").and_then(serde_json::Value::as_i64);
                let query = entry.get("Query").and_then(row_name);
                location
                    .zip(query)
                    .context("a slot override lacks its Location or Query row")
            })
            .collect::<Result<_>>()?,
    };
    let queries: Vec<String> = (0..starting)
        .map(|slot| {
            overrides
                .iter()
                .find(|(location, _)| *location == i64::from(slot))
                .map(|(_, query)| *query)
                .or(template)
                .map(str::to_owned)
                .context("an inventory slot has no query")
        })
        .collect::<Result<_>>()?;
    let mut slots = Vec::new();
    for (query, slot) in queries.iter().zip(0..) {
        let query_row = tables
            .row_index(
                TAG_QUERIES_TABLE,
                &query.encode_utf16().collect::<Vec<u16>>(),
            )?
            .and_then(|row| u16::try_from(row).ok())
            .with_context(|| format!("{TAG_QUERIES_TABLE} lacks a slot query"))?;
        slots.push(InventorySlot::empty(query_row, slot));
    }
    Ok(PawnInventory {
        info_row: u8::try_from(index).context("an inventory info row past 255")?,
        current_weight: None,
        slots: FastArrayContent {
            replication_key: INVENTORY_SLOTS_REPLICATION_KEY,
            items: slots,
        },
    })
}

/// A saved container manager's inventories (`C016`, F-866, U-075), in list
/// order: each one's [`info_inventory`] with its saved items in their slots,
/// each item's `D_ItemsStatic` row, dynamic data and by-name stats, plus
/// `CurrentWeight` when `bInventoryWantsTick` is set and recalculates
/// non-zero. The slots' replication key counts one more change per item, as
/// captured. Fails on a list with empty positions (F-651; how the original
/// replicates them is unobserved), an item the native server cannot send yet
/// (a GUID, an owner, alterations, living slots or an indexed stat), a ticked
/// container item with custom stats or a nested inventory, a location outside
/// the slots or a row the tables lack.
pub fn container_inventories(
    tables: &mut DataTables,
    saved: &SavedContainerManager,
) -> Result<Vec<PawnInventory>> {
    ensure!(
        usize::try_from(saved.count).ok() == Some(saved.containers.len()),
        "the saved inventory list has empty positions"
    );
    let mut weighable_tags: Option<Vec<String>> = None;
    saved
        .containers
        .iter()
        .zip(0..)
        .map(|(container, expected)| {
            ensure!(
                container.index == expected,
                "the saved inventory list has empty positions"
            );
            let mut inventory = info_inventory(tables, &container.info)?;
            for item in &container.items {
                ensure!(
                    item.guid.is_empty()
                        && item.owner_lookup_id == -1
                        && item.alterations == 0
                        && item.living_slots == 0,
                    "a saved item carries fields the native server does not send yet"
                );
                let row = tables
                    .row_index(ITEMS_STATIC_TABLE, &item.row_name)?
                    .and_then(|row| u16::try_from(row).ok())
                    .with_context(|| format!("{ITEMS_STATIC_TABLE} lacks a saved item"))?;
                let dynamic = item
                    .dynamic
                    .iter()
                    .map(|(property, value)| {
                        u32::try_from(*property)
                            .ok()
                            .filter(|property| {
                                i64::from(*property) <= DYNAMIC_ITEM_PROPERTIES_MAX_VALUE
                            })
                            .map(|property| (property, *value))
                            .context("a saved dynamic item property is out of range")
                    })
                    .collect::<Result<_>>()?;
                let stats = item
                    .stats
                    .iter()
                    .map(|(index, name, value)| {
                        ensure!(*index == -1, "a saved item stat is indexed, not named");
                        Ok((ArchiveText::new(name)?, *value))
                    })
                    .collect::<Result<_>>()?;
                let slot = usize::try_from(item.location)
                    .ok()
                    .and_then(|location| inventory.slots.items.get_mut(location))
                    .context("a saved item lies outside its inventory's slots")?;
                ensure!(slot.item.is_none(), "two saved items share a slot");
                slot.item = Some(SlotItem {
                    row,
                    dynamic,
                    stats,
                });
                inventory.slots.replication_key += 1;
            }
            let mut restored = RestoredContainerInventory::from_c016_restore(
                Some(AUTHORITY_LOCAL_ROLE),
                container.wants_tick,
            );
            if container.wants_tick {
                let tags = match &weighable_tags {
                    Some(tags) => tags,
                    None => weighable_tags.insert(weighable_resource_tags(tables)?),
                };
                let mut ordered_items: Vec<&SavedItem> = container.items.iter().collect();
                ordered_items.sort_unstable_by_key(|item| item.location);
                let slot_inputs: Vec<ItemWeightInput> = ordered_items
                    .into_iter()
                    .map(|item| ticked_item_weight_input(tables, item, tags))
                    .collect::<Result<_>>()?;
                restored.tick_if_enabled(|| {
                    recalculate_inventory_weight(
                        DEFAULT_CDO_CURRENT_WEIGHT,
                        true,
                        true,
                        false,
                        false,
                        &slot_inputs,
                        &[],
                    )
                    .expect("container manager owner is eligible")
                    .current_weight
                });
            }
            inventory.current_weight = restored.initial_open_weight();
            Ok(inventory)
        })
        .collect()
}

/// The most starting slots one inventory may list.
const MAX_STARTING_SLOTS: i32 = 1_024;

/// A fresh player's vitals (F-836): full at the maxima the starting stats
/// grant (`BaseMaximum{Health,Stamina,Oxygen,Water,Food}_+`), the growth
/// row `Player` and biome row 0.
pub fn fresh_player_vitals(tables: &mut DataTables) -> Result<CharacterVitals> {
    let row = tables
        .row(STARTING_STATS_TABLE, STARTING_STATS_ROW)?
        .with_context(|| format!("{STARTING_STATS_TABLE} has no {STARTING_STATS_ROW} row"))?;
    let granted = row
        .get("StatsGranted")
        .and_then(serde_json::Value::as_object)
        .context("the starting stats grant no map")?;
    // Keys are exported as `(Value="<stat row>")`.
    let stat = |name: &str| -> Result<i32> {
        granted
            .get(&format!("(Value=\"{name}\")"))
            .and_then(serde_json::Value::as_i64)
            .and_then(|value| i32::try_from(value).ok())
            .with_context(|| format!("the starting stats grant no {name}"))
    };
    let (health, stamina, oxygen, water, food) = (
        stat("BaseMaximumHealth_+")?,
        stat("BaseMaximumStamina_+")?,
        stat("BaseMaximumOxygen_+")?,
        stat("BaseMaximumWater_+")?,
        stat("BaseMaximumFood_+")?,
    );
    let growth = tables
        .row_index(
            CHARACTER_GROWTH_TABLE,
            &PLAYER_GROWTH_ROW.encode_utf16().collect::<Vec<u16>>(),
        )?
        .with_context(|| format!("{CHARACTER_GROWTH_TABLE} has no {PLAYER_GROWTH_ROW} row"))?;
    Ok(CharacterVitals {
        health,
        max_health: health,
        stamina,
        max_stamina: stamina,
        oxygen,
        max_oxygen: oxygen,
        water,
        max_water: water,
        food,
        max_food: food,
        biome_row: 0,
        growth_row: u8::try_from(growth).context("the growth row past 255")?,
    })
}

/// What a loaded world's preserved prospect adds to the GameState open: its
/// `ReplicatedActiveProspect` from the save's `ProspectInfo`, with
/// `ElapsedTime` from the recorded level clock and no member playing (no
/// connection is matched to a member), as the original showed for its
/// loaded save (F-801); the game mode record's values; and the restored
/// level clock (F-803); and a time-of-day clock from the saved `TimeOfDay`
/// (F-806). The original's rate curve is world data and out of scope: the
/// clock runs at the constant rate whose day lasts the saved positive
/// `SecondsPerGameDay`.
pub struct SavedGameState {
    pub values: BTreeMap<u32, RepValue>,
    pub level_time_elapsed_sec: Option<i32>,
    pub time_of_day: Option<TimeOfDayClock>,
    /// Saved session flags that the data tables do not list (or all of
    /// them without tables); they are not replicated.
    pub unmapped_session_flags: usize,
    /// The saved missions `LockedMissions` lists while their locks last
    /// ([`locked_missions`]); empty without tables.
    pub locked_missions: Vec<LockedMission>,
    /// The saved quest manager state (`C018`) and its faction mission's
    /// `D_FactionMissions` row (F-861); `None` without the record, without
    /// tables or when the table lacks the mission (a skipped manager).
    pub quest_manager: Option<(SavedQuestManager, u8)>,
    /// The saved player history (`C024`), when the save has the record
    /// (F-862).
    pub player_history: Option<Vec<PlayerHistoryEntry>>,
    /// The saved FLOD tile count (`C013`), when the save has the record
    /// (F-914).
    pub flod_tiles: Option<i32>,
    /// The saved container inventories (`C016`, F-866) as sendable
    /// inventories; `None` without the record or without tables (as the
    /// QuestManager, without a skip reason).
    pub inventory_containers: Option<Vec<PawnInventory>>,
    /// Managers whose saved state the native server cannot send yet, with
    /// the reason; they stay closed.
    pub skipped_managers: Vec<(ManagerSlot, String)>,
}

/// The saved missions `AIcarusGameModeSurvival::UpdateLockedMissions`
/// keeps (F-834), in saved order: those whose status (its low byte) is
/// neither 2 nor 3 and whose `D_FactionMissions` row does not clear
/// [`RETRY_TIMEOUT_FIELD`]. Missions the table lacks are skipped, as by the
/// original; so are rows past 255, which no 1-byte enumerator names.
pub fn locked_missions(
    history: &[MissionHistoryRecord],
    tables: &mut DataTables,
) -> Result<Vec<LockedMission>> {
    let mut missions = Vec::new();
    for record in history {
        if matches!(record.status.to_le_bytes()[0], 2 | 3) {
            continue;
        }
        let Some(index) = tables.row_index(FACTION_MISSIONS_TABLE, &record.mission)? else {
            continue;
        };
        if tables.row_bool(FACTION_MISSIONS_TABLE, RETRY_TIMEOUT_FIELD, index)? == Some(false) {
            continue;
        }
        if let Ok(row) = u8::try_from(index) {
            missions.push(LockedMission {
                row,
                end_time: record.end_time,
            });
        }
    }
    Ok(missions)
}

/// `D_ProspectList`: a prospect row's `WorldStatList` names the
/// `D_ProspectStats` rows whose `WorldStats` the world starts with.
pub const PROSPECT_LIST_TABLE: &str = "D_ProspectList";
pub const PROSPECT_STATS_TABLE: &str = "D_ProspectStats";
/// The world stats `AIcarusGameStateSurvival::UpdateIsOpenWorldProspect`
/// reads for `bIsOpenWorldProspect` and `bIsOutpostProspect` (F-911).
const OPEN_WORLD_STAT: &str = "WorldIsOpenWorldProspect_?";
const OUTPOST_STAT: &str = "WorldIsOutpostProspect_?";

/// The GameState's `bIsOpenWorldProspect` and `bIsOutpostProspect` for the
/// prospect row `prospect` (F-911): each is set when the `WorldStats` of
/// the row's `WorldStatList` entries grant its stat a positive total, as
/// the original sets them from the world's stats. Without either table, or
/// for a prospect the table lacks, neither is set; a malformed entry fails.
pub fn prospect_world_flags(tables: &mut DataTables, prospect: &[u16]) -> Result<[bool; 2]> {
    if !tables.has_table(PROSPECT_LIST_TABLE) || !tables.has_table(PROSPECT_STATS_TABLE) {
        return Ok([false; 2]);
    }
    let Some(row) = tables.row(PROSPECT_LIST_TABLE, &String::from_utf16_lossy(prospect))? else {
        return Ok([false; 2]);
    };
    let listed: Vec<String> = match row.get("WorldStatList") {
        None => Vec::new(),
        Some(list) => list
            .as_array()
            .context("a prospect's WorldStatList is not an array")?
            .iter()
            .map(|entry| row_name(entry).map(str::to_owned))
            .collect::<Option<_>>()
            .context("a WorldStatList entry names no row")?,
    };
    let mut totals = [0_i64; 2];
    for name in listed {
        let Some(stats) = tables.row(PROSPECT_STATS_TABLE, &name)? else {
            continue;
        };
        let Some(granted) = stats.get("WorldStats") else {
            continue;
        };
        let granted = granted
            .as_object()
            .context("a prospect stats row's WorldStats is not a map")?;
        // Keys are exported as `(Value="<stat row>")`.
        for (total, stat) in totals.iter_mut().zip([OPEN_WORLD_STAT, OUTPOST_STAT]) {
            if let Some(value) = granted.get(&format!("(Value=\"{stat}\")")) {
                *total += value
                    .as_i64()
                    .context("a world stat grant is not an integer")?;
            }
        }
    }
    Ok(totals.map(|total| total > 0))
}

/// With the official server's data tables (F-828), the saved session flags
/// also replicate as `SessionFlags` row indices in saved order (F-830), the
/// saved mission history yields the [`locked_missions`] (F-834) and the
/// prospect's row sets the open-world and outpost flags
/// ([`prospect_world_flags`], F-911).
pub fn saved_game_state(
    section: &PreservedSection,
    mut tables: Option<&mut DataTables>,
) -> Result<SavedGameState> {
    let payload = section.decode()?.to_inner_payload()?;
    let saved = crate::prospect::saved_prospect_from_payload(&payload)?;
    let level_time = saved
        .game_mode
        .as_ref()
        .map(|mode| mode.level_time_elapsed_sec);
    let mut values = active_prospect_values(
        &saved
            .info
            .for_session(level_time.unwrap_or(saved.info.elapsed_time), |_| false),
    )?;
    let mut unmapped_session_flags = 0;
    let mut locked = Vec::new();
    if let Some(mode) = &saved.game_mode {
        values.extend(saved_game_mode_values(mode));
        let mut indices = Vec::new();
        match tables.as_deref_mut() {
            Some(tables) => {
                locked = locked_missions(&mode.mission_history, tables)?;
                for name in &mode.session_flags {
                    match tables
                        .row_index(SESSION_FLAGS_TABLE, name)?
                        .and_then(|index| u8::try_from(index).ok())
                    {
                        Some(index) => indices.push(index),
                        None => unmapped_session_flags += 1,
                    }
                }
            }
            None => unmapped_session_flags = mode.session_flags.len(),
        }
        if !indices.is_empty() {
            values.insert(handle::SESSION_FLAGS, session_flags_value(&indices));
        }
    }
    if let Some(tables) = tables.as_deref_mut() {
        let [open_world, outpost] = prospect_world_flags(tables, &saved.info.prospect_dt_key)?;
        for (handle, set) in [
            (handle::IS_OPEN_WORLD_PROSPECT, open_world),
            (handle::IS_OUTPOST_PROSPECT, outpost),
        ] {
            if set {
                values.insert(handle, RepValue::Bool(true));
            }
        }
    }
    let time_of_day = saved
        .game_mode
        .as_ref()
        .filter(|mode| mode.seconds_per_game_day > 0)
        .map(|mode| {
            RateCurve::for_seconds_per_game_day(mode.seconds_per_game_day)
                .map(|curve| TimeOfDayClock::new(curve, mode.time_of_day))
        })
        .transpose()?;
    let mut skipped_managers = Vec::new();
    let inventory_containers = match (&saved.inventory_containers, tables.as_deref_mut()) {
        (Some(containers), Some(tables)) => match container_inventories(tables, containers) {
            Ok(inventories) => Some(inventories),
            Err(error) => {
                skipped_managers.push((ManagerSlot::InventoryContainer, format!("{error:#}")));
                None
            }
        },
        _ => None,
    };
    let quest_manager = match (saved.quest_manager, tables) {
        (Some(quest), Some(tables)) => {
            let row = tables
                .row_index(FACTION_MISSIONS_TABLE, &quest.faction_mission)?
                .and_then(|index| u8::try_from(index).ok());
            if row.is_none() {
                skipped_managers.push((
                    ManagerSlot::Quest,
                    format!(
                        "the saved faction mission is not a 1-byte {FACTION_MISSIONS_TABLE} row"
                    ),
                ));
            }
            row.map(|row| (quest, row))
        }
        _ => None,
    };
    Ok(SavedGameState {
        values,
        level_time_elapsed_sec: level_time,
        time_of_day,
        unmapped_session_flags,
        locked_missions: locked,
        quest_manager,
        player_history: saved.player_history,
        flod_tiles: saved.flod_tiles,
        inventory_containers,
        skipped_managers,
    })
}

/// Add this server's save settings to `lab`'s GameState (F-810): its
/// autosave interval as `TimeBetweenSaves` and as the state-save interval
/// of a locally saving server's `NetworkingStatus`, which then lists the
/// joined players.
pub fn apply_server_settings(lab: &mut PeerLab, autosave_seconds: u64) -> Result<()> {
    let interval = autosave_seconds as f32;
    lab.extend_game_state_values(BTreeMap::from([(
        handle::TIME_BETWEEN_SAVES,
        RepValue::Float(interval.to_bits()),
    )]))?;
    lab.enable_networking_status(NetworkingStatus::local(interval))
}

/// What [`apply_saved_game_state`] replicated beyond the values.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AppliedGameState {
    /// Saved session flags that could not be replicated.
    pub unmapped_session_flags: usize,
    /// Missions `LockedMissions` lists while their locks last.
    pub locked_missions: usize,
    /// The managers the save's state opened, with their channels, in
    /// opening order (F-861, F-862, F-866).
    pub saved_managers: Vec<(ManagerSlot, u32)>,
    /// Managers whose saved state could not be sent, with the reason.
    pub skipped_managers: Vec<(ManagerSlot, String)>,
}

/// Add a loaded world's [`saved_game_state`] to `lab`'s GameState open,
/// and open those of `saved_managers` (the release's plans) whose state
/// the save supplies after the other managers: the QuestManager with the
/// saved quest state (F-861), the InventoryContainerManager with the saved
/// container inventories (F-866), the PlayerHistoryTracker with the saved
/// history (F-862) and the FLOD manager with the saved tile count (F-914).
pub fn apply_saved_game_state(
    lab: &mut PeerLab,
    section: &PreservedSection,
    tables: Option<&mut DataTables>,
    saved_managers: Vec<(ManagerSlot, ManagerOpenPlan)>,
) -> Result<AppliedGameState> {
    let mut saved = saved_game_state(section, tables)?;
    // The managers join last, after every other step that can fail.
    let mut manager_plans = Vec::new();
    for (slot, plan) in saved_managers {
        let plan = match slot {
            ManagerSlot::Quest => match &saved.quest_manager {
                Some((quest, row)) => plan.with_quest_state(quest, *row)?,
                None => continue,
            },
            ManagerSlot::InventoryContainer => match &saved.inventory_containers {
                Some(inventories) => match plan.with_container_inventories(inventories.clone()) {
                    Ok(plan) => plan,
                    Err(error) => {
                        saved.skipped_managers.push((slot, format!("{error:#}")));
                        continue;
                    }
                },
                None => continue,
            },
            ManagerSlot::PlayerHistory => match &saved.player_history {
                Some(history) => plan.with_player_history(history)?,
                None => continue,
            },
            ManagerSlot::Flod => match saved.flod_tiles {
                Some(count) => match plan.with_flod_tiles(count) {
                    Ok(plan) => plan,
                    Err(error) => {
                        saved.skipped_managers.push((slot, format!("{error:#}")));
                        continue;
                    }
                },
                None => continue,
            },
            _ => continue,
        };
        manager_plans.push((slot, plan));
    }
    lab.extend_game_state_values(saved.values)?;
    let locked_missions = saved.locked_missions.len();
    if let Some(level_time) = saved.level_time_elapsed_sec {
        lab.set_game_state_level_time(level_time)?;
        if locked_missions > 0 {
            lab.enable_locked_missions(saved.locked_missions)?;
        }
    }
    if let Some(clock) = saved.time_of_day {
        lab.enable_time_of_day(clock)?;
    }
    let saved_managers = manager_plans
        .into_iter()
        .map(|(slot, plan)| Ok((slot, lab.add_manager_open(slot, plan)?)))
        .collect::<Result<_>>()?;
    Ok(AppliedGameState {
        unmapped_session_flags: saved.unmapped_session_flags,
        locked_missions,
        saved_managers,
        skipped_managers: saved.skipped_managers,
    })
}

/// Session counts kept after [`teardown`] so a repeated call and the final
/// record report the same values.
#[derive(Resource, Clone, Debug)]
struct ClosedSessions(SessionCounts);

/// Peer-lab parameters for `network`: admission enabled with `max_players`
/// as the occupancy limit. The peer table holds twice the admitted slots so
/// sessions beyond capacity can still complete a handshake and receive the
/// admission Failure instead of silence.
pub fn peer_lab_config(network: &NetworkConfig) -> PeerLabConfig {
    let defaults = PeerLabAdmission::default();
    let max_players = i32::try_from(network.max_players).unwrap_or(i32::MAX);
    let admission = PeerLabAdmission {
        policy: AdmissionPolicy {
            max_players,
            max_spectators: SPECTATOR_SLOTS as i32,
            ..defaults.policy
        },
        ..defaults
    };
    PeerLabConfig {
        max_peers: 2 * (network.max_players + SPECTATOR_SLOTS) as usize,
        idle_timeout: network.session_timeout(),
        admission: Some(admission),
        ..PeerLabConfig::default()
    }
}

/// A bound session socket and the server's own release pin its Hello
/// version check uses, if one is configured.
pub struct BoundNetwork {
    pub lab: PeerLab,
    pub release: Option<ReleasePin>,
    /// The official server's data tables, when `data_pak` is set.
    pub tables: Option<DataTables>,
    /// The release's plans of the managers that open only with a save's
    /// state ([`apply_saved_game_state`]).
    pub saved_managers: Vec<(ManagerSlot, ManagerOpenPlan)>,
}

/// Load and validate the configured release profile (bounded regular-file
/// read), or `None` when `release_profile` is unset.
pub fn release_profile(network: &NetworkConfig) -> Result<Option<ReleaseProfile>> {
    network
        .release_profile
        .as_deref()
        .map(|path| {
            read_configuration(path)
                .and_then(|text| ReleaseProfile::parse(&text))
                .with_context(|| format!("load network.release_profile {}", path.display()))
        })
        .transpose()
}

/// Open a data pak and load the tables `run` reads from it.
fn load_data_tables(path: &std::path::Path) -> Result<DataTables> {
    let mut tables = DataTables::open(path)?;
    tables.rows(SESSION_FLAGS_TABLE)?;
    tables.rows(FACTION_MISSIONS_TABLE)?;
    tables.row_bool(FACTION_MISSIONS_TABLE, RETRY_TIMEOUT_FIELD, 0)?;
    pawn_content(Some(&mut tables))?;
    Ok(tables)
}

/// Validate `network`, load its release profile and only then bind its
/// socket, or return `None` when disabled.
pub fn bind(network: &NetworkConfig) -> Result<Option<BoundNetwork>> {
    if !network.enabled {
        return Ok(None);
    }
    network.validate()?;
    let profile = release_profile(network)?;
    // The tables run reads are loaded here, so a pak without them fails
    // before the socket binds rather than at the first apply.
    let mut tables = network
        .data_pak
        .as_deref()
        .map(|path| {
            load_data_tables(path)
                .with_context(|| format!("load network.data_pak {}", path.display()))
        })
        .transpose()?;
    let mut config = peer_lab_config(network);
    if let Some(profile) = &profile {
        config = config.with_release_profile(profile);
    }
    let plan = match (&profile, &config.admission) {
        (Some(profile), Some(admission)) => profile
            .player_controller_open_plan(admission.map_url, SPAWN_LOCATION)
            .context("compose the PlayerController open plan")?,
        _ => None,
    };
    let address = SocketAddrV4::new(network.bind, network.port);
    let mut lab = PeerLab::bind_address(address, config)
        .context("bind the experimental network session socket")?;
    let mut saved_managers = Vec::new();
    if let Some(plan) = plan {
        lab.enable_player_controller_open(plan)?;
        let player_state = match (&profile, &config.admission) {
            (Some(profile), Some(admission)) => profile
                .player_state_open_plan(admission.map_url)
                .context("compose the PlayerState open plan")?,
            _ => None,
        };
        let pawn = match (&profile, &config.admission) {
            (Some(profile), Some(admission)) => profile
                .pawn_open_plan(admission.map_url)
                .context("compose the pawn open plan")?,
            _ => None,
        };
        if let Some(pawn) = pawn {
            lab.enable_pawn_open(pawn, PAWN_CHANNEL_INDEX, pawn_content(tables.as_mut())?)?;
        }
        let game_state = match (&profile, &config.admission) {
            (Some(profile), Some(admission)) => profile
                .game_state_open_plan(admission.map_url)
                .context("compose the GameState open plan")?,
            _ => None,
        };
        if let Some(game_state) = game_state {
            lab.enable_game_state_open(game_state, game_state_values()?, GAME_STATE_CHANNEL_INDEX)?;
            // The server's own frame rate, measured as the original does.
            lab.enable_server_fps(ServerFpsMeter::new(DEFAULT_SAMPLES)?)?;
        }
        if let Some(player_state) = player_state {
            lab.enable_player_state_open(player_state)?;
        }
        // The GameState's managers, after the PlayerState (F-859).
        if lab.has_game_state_open()
            && let (Some(profile), Some(admission)) = (&profile, &config.admission)
        {
            let managers = profile
                .manager_open_plans(admission.map_url)
                .context("compose the manager open plans")?;
            if !managers.is_empty() {
                lab.enable_manager_opens(managers, MANAGER_FIRST_CHANNEL)?;
                saved_managers = profile
                    .saved_manager_open_plans(admission.map_url)
                    .context("compose the saved manager open plans")?;
            }
        }
    }
    Ok(Some(BoundNetwork {
        lab,
        release: profile.map(|profile| profile.pin),
        tables,
        saved_managers,
    }))
}

/// Install a bound lab into the production app; returns its local address.
pub fn install(app: &mut App, lab: PeerLab) -> Result<SocketAddr> {
    let address = lab.local_addr().context("network session socket address")?;
    app.insert_resource(lab).add_plugins(PeerLabPlugin);
    Ok(address)
}

/// Current counts, the counts kept by [`teardown`], or `None` when the
/// session layer was never installed.
pub fn session_counts(world: &World) -> Option<SessionCounts> {
    world
        .get_resource::<PeerLab>()
        .map(PeerLab::session_counts)
        .or_else(|| {
            world
                .get_resource::<ClosedSessions>()
                .map(|closed| closed.0.clone())
        })
}

/// Stop admitting new sessions; live sessions are kept. Idempotent.
pub fn stop_admission(world: &mut World) {
    if let Some(mut lab) = world.get_resource_mut::<PeerLab>() {
        lab.stop_admission();
    }
}

/// Stop admission, notify and remove every session and drop the socket.
/// Idempotent: a repeated call returns the same final counts.
pub fn teardown(world: &mut World) -> Option<SessionCounts> {
    if let Some(mut lab) = world.remove_resource::<PeerLab>() {
        lab.shutdown();
        let counts = lab.session_counts();
        drop(lab);
        world.insert_resource(ClosedSessions(counts));
    }
    world
        .get_resource::<ClosedSessions>()
        .map(|closed| closed.0.clone())
}
