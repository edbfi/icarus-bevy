//! The save's `ProspectInfo` and level clock typed, and the GameState's
//! `ReplicatedActiveProspect` mapped from them (F-738, F-801), over an
//! independently framed authored envelope (no supplied save).
#[path = "pak_writer/mod.rs"]
mod pak_writer;

use std::collections::BTreeMap;
use std::io::Write;

use base64::{Engine, engine::general_purpose::STANDARD};
use flate2::{Compression, write::ZlibEncoder};
use icarus_bevy::experimental::control_messages::ArchiveText;
use icarus_bevy::experimental::game_state_open::{
    LockedMission, active_prospect_values, handle, level_time_elapsed_sec, locked_missions_value,
    saved_game_mode_values,
};
use icarus_bevy::experimental::property_tree::{PropertyTree, TreeLimits};
use icarus_bevy::experimental::prospect_info::{
    AssociatedMember, MissionHistoryRecord, ProspectInfo, SavedGameMode,
};
use icarus_bevy::experimental::recorders::recorder_class_label;
use icarus_bevy::experimental::rep_layout::RepValue;
use icarus_bevy::experimental::tagged::TagProfile;
use icarus_bevy::experimental::typed_value::TypedTree;
use icarus_bevy::preservation::{PreserveLimits, PreservedSection};
use icarus_bevy::prospect::{read_saved_prospect, saved_prospect_from_payload};
use serde_json::json;
use sha1::{Digest, Sha1};

const GAME_MODE: &str = "/Script/Icarus.GameModeStateRecorderComponent";
const QUEST_MANAGER: &str = "/Script/Icarus.IcarusQuestManagerRecorderComponent";
const PLAYER_HISTORY: &str = "/Script/Icarus.PlayerHistoryRecorderComponent";
const CONTAINER_MANAGER: &str = "/Script/Icarus.IcarusContainerManagerRecorderComponent";

fn fstring(value: &str) -> Vec<u8> {
    if value.is_empty() {
        0_i32.to_le_bytes().to_vec()
    } else if value.is_ascii() {
        [
            i32::try_from(value.len() + 1)
                .unwrap()
                .to_le_bytes()
                .as_slice(),
            value.as_bytes(),
            &[0],
        ]
        .concat()
    } else {
        let units: Vec<u16> = value.encode_utf16().chain([0]).collect();
        let mut bytes = (-i32::try_from(units.len()).unwrap())
            .to_le_bytes()
            .to_vec();
        bytes.extend(units.iter().flat_map(|unit| unit.to_le_bytes()));
        bytes
    }
}

fn tag(name: &str, kind: &str, extras: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        fstring(name).as_slice(),
        &fstring(kind),
        &i32::try_from(payload.len()).unwrap().to_le_bytes(),
        &0_i32.to_le_bytes(),
        extras,
        payload,
    ]
    .concat()
}

fn string(name: &str, value: &str) -> Vec<u8> {
    tag(name, "StrProperty", &[0], &fstring(value))
}

fn int(name: &str, value: i32) -> Vec<u8> {
    tag(name, "IntProperty", &[0], &value.to_le_bytes())
}

fn boolean(name: &str, value: bool) -> Vec<u8> {
    tag(name, "BoolProperty", &[u8::from(value), 0], &[])
}

fn enumerator(name: &str, enum_name: &str, value: &str) -> Vec<u8> {
    tag(
        name,
        "EnumProperty",
        &[fstring(enum_name), vec![0]].concat(),
        &fstring(&format!("{enum_name}::{value}")),
    )
}

fn struct_tag(name: &str, type_name: &str, payload: &[u8]) -> Vec<u8> {
    tag(
        name,
        "StructProperty",
        &[fstring(type_name), vec![0; 17]].concat(),
        payload,
    )
}

fn struct_array(name: &str, type_name: &str, elements: &[Vec<u8>]) -> Vec<u8> {
    let shared = tag(
        name,
        "StructProperty",
        &[fstring(type_name), vec![0; 17]].concat(),
        &elements.concat(),
    );
    let count = i32::try_from(elements.len()).unwrap().to_le_bytes();
    tag(
        name,
        "ArrayProperty",
        &[fstring("StructProperty"), vec![0]].concat(),
        &[count.as_slice(), &shared].concat(),
    )
}

fn end() -> Vec<u8> {
    fstring("None")
}

fn member(name: &str, user: &str, slot: i32, status: &str, playing: bool) -> Vec<u8> {
    [
        string("AccountName", name),
        string("CharacterName", name),
        string("UserID", user),
        int("ChrSlot", slot),
        int("Experience", 0),
        enumerator("Status", "EProspectLocation", status),
        boolean("Settled", false),
        boolean("IsCurrentlyPlaying", playing),
        end(),
    ]
    .concat()
}

/// An authored `ProspectInfo`: some fields at defaults (absent, empty or
/// zero), a wide member name, enumerators by name.
fn prospect_info() -> Vec<u8> {
    [
        string("ProspectID", "lab-prospect"),
        string("ClaimedAccountID", "account-1"),
        int("ClaimedAccountCharacter", 2),
        string("ProspectDTKey", "Lab_Key"),
        string("LobbyName", ""),
        tag("ExpireTime", "Int64Property", &[0], &(-1_i64).to_le_bytes()),
        enumerator("ProspectState", "EProspectState", "Active"),
        struct_array(
            "AssociatedMembers",
            "AssociatedMemberInfo",
            &[
                member("First", "user-1", 1, "Prospect_Conifer", true),
                member("Ærø", "user-2", 0, "Hab", false),
            ],
        ),
        int("Reward", 0),
        enumerator("Difficulty", "EMissionDifficulty", "Hard"),
        boolean("Insurance", false),
        boolean("NoRespawns", true),
        int("ElapsedTime", 100),
        int("SelectedDropPoint", 3),
        struct_array(
            "CustomSettings",
            "CustomGameSetting",
            &[[
                string("SettingRowName", "Lab_Setting"),
                int("SettingValue", 7),
                end(),
            ]
            .concat()],
        ),
        end(),
    ]
    .concat()
}

/// The fixture's saved missions: (row name, status, end time).
const MISSION_HISTORY: [(&str, i32, i32); 5] = [
    ("Mission_A", 1, 40),
    ("Dyn_X", 1, 50),
    ("Mission_B", 2, 60),
    ("Mission_Unknown", 1, 70),
    ("Mission_C", 0, -1),
];

/// An authored `D_FactionMissions`: `Dyn_X` clears the retry timeout,
/// `Mission_Unknown` is missing.
fn faction_missions_json() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "RowStruct": "/Script/Icarus.FactionMission",
        "Rows": [
            {"Name": "Mission_Z"},
            {"Name": "Mission_A"},
            {"Name": "Dyn_X", "bUseOpenWorldRetryTimeout": false},
            {"Name": "Mission_C", "bUseOpenWorldRetryTimeout": true},
            {"Name": "Mission_B"},
        ],
    }))
    .unwrap()
}

fn game_mode_recorder(level_time: i32) -> Vec<u8> {
    let component = [
        struct_tag(
            "GameModeRecord",
            "GameModeRecord",
            &[
                int("GameStateSeed", 4_242),
                tag("TimeOfDay", "FloatProperty", &[0], &12.5_f32.to_le_bytes()),
                tag(
                    "ProspectGameTime",
                    "FloatProperty",
                    &[0],
                    &99.5_f32.to_le_bytes(),
                ),
                int("SecondsPerGameDay", 3_600),
                int("LevelTimeElapsedSec", level_time),
                tag(
                    "SessionFlagRecords",
                    "ArrayProperty",
                    &[fstring("NameProperty"), vec![0]].concat(),
                    &[
                        3_i32.to_le_bytes().to_vec(),
                        fstring("Flag_B"),
                        fstring("Flag_Unknown"),
                        fstring("Flag_A"),
                    ]
                    .concat(),
                ),
                end(),
            ]
            .concat(),
        ),
        int("DynamicQuestSeed", 77),
        struct_array(
            "MissionHistory",
            "MissionHistoryRecord",
            &MISSION_HISTORY
                .iter()
                .map(|(mission, status, end_time)| {
                    [
                        string("Mission", mission),
                        int("Status", *status),
                        int("MissionEndTime", *end_time),
                        end(),
                    ]
                    .concat()
                })
                .collect::<Vec<_>>(),
        ),
    ]
    .concat();
    recorder(GAME_MODE, &component)
}

/// A recorder blob of `class` whose component holds tagged `fields`.
fn recorder(class: &str, fields: &[u8]) -> Vec<u8> {
    let component = [fields, &end(), &[0; 4]].concat();
    let count = i32::try_from(component.len()).unwrap().to_le_bytes();
    [
        string("ComponentClassName", class),
        tag(
            "BinaryData",
            "ArrayProperty",
            &[fstring("ByteProperty"), vec![0]].concat(),
            &[count.as_slice(), &component].concat(),
        ),
        end(),
    ]
    .concat()
}

/// A quest manager recorder (`C018`): faction mission `Mission_C` (row 3
/// of `faction_missions_json`), difficulty 2, quests running, the initial
/// quest actor 9001 and two persistent actors, one without a valid GUID.
fn quest_manager_recorder() -> Vec<u8> {
    let relevant = |class: &str, key: &str, uid: i32| {
        [
            string("RelevantActorClassName", class),
            string("Key", key),
            int("RelevantActorIcarusUID", uid),
            end(),
        ]
        .concat()
    };
    let fields = [
        tag(
            "FactionMissionName",
            "NameProperty",
            &[0],
            &fstring("Mission_C"),
        ),
        struct_tag(
            "InitialQuestRecord",
            "InitialQuestRecord",
            &[
                string("QuestActorName", "BPQ_Lab_C"),
                int("RelevantActorIcarusUID", 9_001),
                end(),
            ]
            .concat(),
        ),
        boolean("bMissionComplete", false),
        boolean("bRunQuests", true),
        int("DynamicQuestDifficulty", 2),
        tag(
            "DynamicQuestDelay",
            "FloatProperty",
            &[0],
            &0.0_f32.to_le_bytes(),
        ),
        struct_array(
            "RelevantPersistentActorRecords",
            "RelevantQuestActorRecord",
            &[
                relevant("BP_Lab_Base_C", "LAB_BASE", 501),
                relevant("BP_Lab_Gone_C", "LAB_GONE", -1),
            ],
        ),
        struct_array(
            "RelevantPersistentCharacterRecords",
            "RelevantQuestActorRecord",
            &[],
        ),
        int("IcarusActorGUID", 77),
    ]
    .concat();
    recorder(QUEST_MANAGER, &fields)
}

fn payload(info: &[u8], recorders: &[Vec<u8>]) -> Vec<u8> {
    let shared = tag(
        "StateRecorderBlobs",
        "StructProperty",
        &[fstring("StateRecorderBlob"), vec![0; 17]].concat(),
        &recorders.concat(),
    );
    let count = i32::try_from(recorders.len()).unwrap().to_le_bytes();
    [
        int("Version", 2),
        struct_tag("ProspectInfo", "ProspectInfo", info),
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &[fstring("StructProperty"), vec![0]].concat(),
            &[count.as_slice(), &shared].concat(),
        ),
        end(),
    ]
    .concat()
}

fn envelope(payload: &[u8]) -> Vec<u8> {
    let mut encoder = ZlibEncoder::new(Vec::new(), Compression::fast());
    encoder.write_all(payload).unwrap();
    let compressed = encoder.finish().unwrap();
    let hash = Sha1::digest(payload)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>();
    serde_json::to_vec(&json!({"ProspectBlob": {
        "Hash": hash, "TotalLength": compressed.len(), "DataLength": 0,
        "UncompressedLength": payload.len(), "BinaryBlob": STANDARD.encode(compressed)
    }}))
    .unwrap()
}

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

#[test]
fn the_game_mode_fixture_class_is_c015() {
    assert_eq!(
        recorder_class_label(&units(GAME_MODE)).as_deref(),
        Some("C015")
    );
}

#[test]
fn a_saved_prospect_types_its_info_and_level_clock() {
    let source = envelope(&payload(&prospect_info(), &[game_mode_recorder(150)]));
    let saved = read_saved_prospect(source.as_slice()).unwrap();
    assert_eq!(
        saved.game_mode,
        Some(SavedGameMode {
            game_state_seed: 4_242,
            time_of_day: 12.5,
            prospect_game_time: 99.5,
            seconds_per_game_day: 3_600,
            level_time_elapsed_sec: 150,
            dynamic_quest_seed: 77,
            session_flags: vec![units("Flag_B"), units("Flag_Unknown"), units("Flag_A")],
            mission_history: MISSION_HISTORY
                .iter()
                .map(|(mission, status, end_time)| MissionHistoryRecord {
                    mission: units(mission),
                    status: *status,
                    end_time: *end_time,
                })
                .collect(),
        })
    );
    let info = &saved.info;
    assert_eq!(info.prospect_id, units("lab-prospect"));
    assert_eq!(info.claimed_account_character, 2);
    assert!(info.faction_mission_dt_key.is_empty() && info.lobby_name.is_empty());
    assert_eq!(info.expire_time, -1);
    assert_eq!((info.prospect_state, info.difficulty), (2, 3));
    assert_eq!((info.cost, info.reward), (0, 0));
    assert!(!info.insurance && info.no_respawns);
    assert_eq!((info.elapsed_time, info.selected_drop_point), (100, 3));
    assert_eq!(
        info.associated_members[1],
        AssociatedMember {
            account_name: units("Ærø"),
            character_name: units("Ærø"),
            user_id: units("user-2"),
            chr_slot: 0,
            experience: 0,
            status: 1,
            settled: false,
            is_currently_playing: false,
        }
    );
    assert_eq!(info.associated_members[0].status, 2);
    assert_eq!(info.custom_settings[0].setting_value, 7);
    // Without a game mode recorder there is no level clock.
    let bare = read_saved_prospect(envelope(&payload(&prospect_info(), &[])).as_slice()).unwrap();
    assert_eq!(bare.game_mode, None);
    assert_eq!(bare.info, saved.info);
}

#[test]
fn mistyped_or_unknown_fields_fail_closed() {
    let typed = |fields: &[Vec<u8>]| {
        let stream = [fields.concat(), end()].concat();
        let tree =
            PropertyTree::parse(&stream, TagProfile::MemoryArchive522, TreeLimits::default())
                .unwrap();
        ProspectInfo::from_tree(&TypedTree::from_tree(&tree).unwrap().0)
    };
    // Absent fields read as defaults.
    assert_eq!(typed(&[]).unwrap(), ProspectInfo::default());
    assert_eq!(typed(&[int("Cost", 5)]).unwrap().cost, 5);
    for bad in [
        int("ProspectState", 2),
        enumerator("Difficulty", "EMissionDifficulty", "Brutal"),
        enumerator("ProspectState", "EProspectState", "MaxProspectStates2"),
        int("ProspectID", 1),
        string("ElapsedTime", "100"),
        tag("ExpireTime", "IntProperty", &[0], &1_i32.to_le_bytes()),
        int("NoRespawns", 1),
        int("AssociatedMembers", 1),
    ] {
        assert!(typed(&[bad]).is_err());
    }
    // A member with a mistyped field fails the whole struct.
    let members = struct_array(
        "AssociatedMembers",
        "AssociatedMemberInfo",
        &[[int("UserID", 1), end()].concat()],
    );
    assert!(typed(&[members]).is_err());
}

fn text(value: &str) -> RepValue {
    RepValue::Str(ArchiveText::new(&units(value)).unwrap())
}

#[test]
fn the_active_prospect_maps_nondefault_fields_in_declaration_order() {
    let saved = read_saved_prospect(envelope(&payload(&prospect_info(), &[])).as_slice()).unwrap();
    let session = saved
        .info
        .for_session(150, |member| member.user_id == units("user-2"));
    assert_eq!(session.elapsed_time, 150);
    let playing: Vec<bool> = session
        .associated_members
        .iter()
        .map(|member| member.is_currently_playing)
        .collect();
    assert_eq!(playing, [false, true]);
    let values = active_prospect_values(&session).unwrap();
    let first = handle::ACTIVE_PROSPECT_FIRST;
    // Defaults stay unsent: FactionMissionDTKey, LobbyName, Cost, Reward,
    // Insurance.
    let sent: Vec<u32> = values.keys().map(|handle| handle - first + 1).collect();
    assert_eq!(sent, [1, 2, 3, 4, 7, 8, 9, 12, 14, 15, 16, 17]);
    assert_eq!(values[&first], text("lab-prospect"));
    assert_eq!(values[&(first + 6)], RepValue::Int64(-1));
    assert_eq!(values[&(first + 7)], RepValue::Enum(2));
    assert_eq!(values[&(first + 11)], RepValue::Enum(3));
    assert_eq!(values[&(first + 13)], RepValue::Bool(true));
    assert_eq!(values[&(first + 14)], RepValue::Int(150));
    // Members: every element field, 8 handles each.
    let RepValue::DynamicArray(members) = &values[&(first + 8)] else {
        panic!("members array");
    };
    assert_eq!(members.count, 2);
    let entries: BTreeMap<u32, RepValue> = members
        .stream
        .entries
        .iter()
        .map(|entry| (entry.handle, entry.value.clone()))
        .collect();
    assert_eq!(entries.len(), 16);
    assert_eq!(entries[&9], text("Ærø"));
    assert_eq!(entries[&14], RepValue::Enum(1));
    assert_eq!(entries[&8], RepValue::Bool(false));
    assert_eq!(entries[&16], RepValue::Bool(true));
    let RepValue::DynamicArray(settings) = &values[&(first + 16)] else {
        panic!("settings array");
    };
    assert_eq!(settings.count, 1);
    assert_eq!(settings.stream.entries[1].value, RepValue::Int(7));
    // An empty prospect sends nothing.
    assert!(
        active_prospect_values(&ProspectInfo::default())
            .unwrap()
            .is_empty()
    );
}

#[test]
fn game_mode_values_map_nondefaults_and_the_level_clock_advances() {
    let mode = SavedGameMode {
        game_state_seed: 4_242,
        time_of_day: 12.5,
        prospect_game_time: 0.0,
        seconds_per_game_day: 3_600,
        level_time_elapsed_sec: 150,
        dynamic_quest_seed: 0,
        session_flags: Vec::new(),
        mission_history: Vec::new(),
    };
    assert_eq!(
        saved_game_mode_values(&mode),
        BTreeMap::from([
            (handle::SEED, RepValue::Int(4_242)),
            (handle::SEED_INITIALISED, RepValue::Bool(true)),
            (handle::TIME_OF_DAY, RepValue::Float(12.5_f32.to_bits())),
            (handle::SECONDS_PER_GAME_DAY, RepValue::Int(3_600)),
        ])
    );
    assert_eq!(level_time_elapsed_sec(150, 14.9), 164);
    assert_eq!(level_time_elapsed_sec(150, -1.0), 150);
    assert_eq!(level_time_elapsed_sec(i32::MAX, 5.0), i32::MAX);
}

/// The GameState values a preserved world adds for this fixture.
fn expected_saved_values() -> BTreeMap<u32, RepValue> {
    let saved = saved_prospect_from_payload(&payload(&prospect_info(), &[game_mode_recorder(150)]))
        .unwrap();
    let mut expected = active_prospect_values(&saved.info.for_session(150, |_| false)).unwrap();
    expected.extend(saved_game_mode_values(&saved.game_mode.unwrap()));
    expected
}

#[test]
fn a_preserved_world_supplies_the_session_game_state() {
    let inner = payload(&prospect_info(), &[game_mode_recorder(150)]);
    let section = PreservedSection::from_inner_payload(&inner, PreserveLimits::default()).unwrap();
    let saved = icarus_bevy::network::saved_game_state(&section, None).unwrap();
    assert_eq!(saved.values, expected_saved_values());
    // Without the server's data tables no session flag replicates.
    assert_eq!(saved.unmapped_session_flags, 3);
    assert_eq!(saved.level_time_elapsed_sec, Some(150));
    // The saved time of day advances at the saved day length's rate (F-806).
    let mut clock = saved.time_of_day.expect("a time-of-day clock");
    assert_eq!(clock.time_of_day(), 12.5);
    assert_eq!(clock.seconds_per_game_day(), 3_600);
    clock.advance(10.0);
    assert!((clock.time_of_day() - (12.5 + 10.0 * 1_440.0 / 3_600.0)).abs() < 1e-4);
    // Without the game mode recorder, the saved elapsed time stands and no
    // game mode values or level clock are added.
    let bare = PreservedSection::from_inner_payload(
        &payload(&prospect_info(), &[]),
        PreserveLimits::default(),
    )
    .unwrap();
    let saved = icarus_bevy::network::saved_game_state(&bare, None).unwrap();
    assert_eq!(
        saved.values[&(handle::ACTIVE_PROSPECT_FIRST + 14)],
        RepValue::Int(100)
    );
    assert!(!saved.values.contains_key(&handle::SEED_INITIALISED));
    assert_eq!(saved.level_time_elapsed_sec, None);
    assert!(saved.time_of_day.is_none());
}

/// With the official server's data tables, the saved session flags it lists
/// replicate as `SessionFlags` row indices in saved order; others are
/// counted and left out (F-830).
#[test]
fn saved_session_flags_replicate_as_data_table_row_indices() {
    let root = tempfile::TempDir::new().unwrap();
    let pak = root.path().join("data.pak");
    std::fs::write(
        &pak,
        pak_writer::write_pak(&[
            (
                "Flags/D_SessionFlags.json",
                &pak_writer::table_json(&["Flag_A", "Flag_X", "Flag_B"]),
                true,
            ),
            (
                "Factions/D_FactionMissions.json",
                &faction_missions_json(),
                false,
            ),
        ]),
    )
    .unwrap();
    let mut tables = icarus_bevy::data_pak::DataTables::open(&pak).unwrap();
    let inner = payload(&prospect_info(), &[game_mode_recorder(150)]);
    let section = PreservedSection::from_inner_payload(&inner, PreserveLimits::default()).unwrap();
    let saved = icarus_bevy::network::saved_game_state(&section, Some(&mut tables)).unwrap();
    assert_eq!(saved.unmapped_session_flags, 1);
    let mut expected = expected_saved_values();
    expected.insert(
        handle::SESSION_FLAGS,
        icarus_bevy::experimental::game_state_open::session_flags_value(&[2, 0]),
    );
    assert_eq!(saved.values, expected);
    let RepValue::DynamicArray(flags) = &saved.values[&handle::SESSION_FLAGS] else {
        panic!("session flags array");
    };
    assert_eq!(flags.count, 2);
    assert_eq!(flags.stream.entries[0].value, RepValue::Byte(2));
    assert_eq!(flags.stream.entries[1].value, RepValue::Byte(0));
    // Locked missions (F-834): status 2 and 3, rows that clear the retry
    // timeout and missions the table lacks are left out; order is saved.
    assert_eq!(saved.locked_missions, expected_locked_missions());
}

fn expected_locked_missions() -> Vec<LockedMission> {
    vec![
        LockedMission {
            row: 1,
            end_time: 40,
        },
        LockedMission {
            row: 3,
            end_time: -1,
        },
    ]
}

/// Two authored player history entries (`C024`), the second with a
/// non-ASCII name.
const HISTORY: [(&str, i32, &str); 2] = [("user-1", 0, "Ada"), ("user-2", 3, "Bjørn")];

fn player_history_recorder() -> Vec<u8> {
    recorder(
        PLAYER_HISTORY,
        &[
            struct_array(
                "SavedHistoryData",
                "PlayerHistoryEntryRecord",
                &HISTORY
                    .iter()
                    .map(|(user, slot, name)| {
                        [
                            string("UserID", user),
                            int("ChrSlot", *slot),
                            string("CachedCharacterName", name),
                            end(),
                        ]
                        .concat()
                    })
                    .collect::<Vec<_>>(),
            ),
            int("IcarusActorGUID", 78),
        ]
        .concat(),
    )
}

fn name(name: &str, value: &str) -> Vec<u8> {
    tag(name, "NameProperty", &[0], &fstring(value))
}

/// A saved item's fields the fixtures vary.
struct ItemFields {
    location: i32,
    guid: &'static str,
    owner: i32,
    dynamic_property: i32,
    stat_index: i32,
}

impl ItemFields {
    /// `Tonic` at `location` with a stack of 4 and a named `Potency` stat
    /// of 3, no GUID and no owner.
    fn at(location: i32) -> Self {
        Self {
            location,
            guid: "",
            owner: -1,
            dynamic_property: 7,
            stat_index: -1,
        }
    }
}

fn item_with(fields: &ItemFields, extra: &[Vec<u8>]) -> Vec<u8> {
    [
        int("Location", fields.location),
        name("ItemStaticData", "Tonic"),
        string("ItemGuid", fields.guid),
        int("ItemOwnerLookupId", fields.owner),
        struct_array(
            "DynamicData",
            "InventorySlotDynamicData",
            &[[
                int("Index", fields.dynamic_property),
                int("Value", 4),
                end(),
            ]
            .concat()],
        ),
        struct_array(
            "AdditionalStats",
            "InventorySlotStatData",
            &[[
                int("Index", fields.stat_index),
                string("Name", "Potency"),
                int("Value", 3),
                end(),
            ]
            .concat()],
        ),
        extra.concat(),
        end(),
    ]
    .concat()
}

/// One saved item at `location` ([`ItemFields::at`]) plus `extra` fields.
fn saved_item(location: i32, extra: &[Vec<u8>]) -> Vec<u8> {
    item_with(&ItemFields::at(location), extra)
}

/// A container manager recorder (`C016`) of `count` positions holding the
/// `containers` (index, info row, items).
fn container_manager_recorder(count: i32, containers: &[(i32, &str, Vec<Vec<u8>>)]) -> Vec<u8> {
    let elements: Vec<Vec<u8>> = containers
        .iter()
        .map(|(index, info, items)| {
            [
                int("InventoryIndex", *index),
                name("InventoryInfo", info),
                struct_tag(
                    "InventorySaveData",
                    "InventorySaveData",
                    &[
                        struct_array("Slots", "InventorySlotSaveData", items),
                        int("InventoryID", -1),
                        end(),
                    ]
                    .concat(),
                ),
                boolean("bInventoryWantsTick", false),
                end(),
            ]
            .concat()
        })
        .collect();
    recorder(
        CONTAINER_MANAGER,
        &[
            int("NumInventories", count),
            struct_array(
                "SavedInventoryContainers",
                "SavedInventoryContainerData",
                &elements,
            ),
        ]
        .concat(),
    )
}

/// The authored fixture: `Hands` (3 slots) with an item in slot 1, then an
/// empty `Bag` (no slots).
fn two_containers() -> Vec<u8> {
    container_manager_recorder(
        2,
        &[
            (0, "Hands", vec![saved_item(1, &[])]),
            (1, "Bag", Vec::new()),
        ],
    )
}

/// The official tables the container fixture maps through: the player
/// tables plus an authored `D_ItemsStatic`.
fn container_tables(root: &std::path::Path) -> icarus_bevy::data_pak::DataTables {
    let pak = root.join("data.pak");
    std::fs::write(
        &pak,
        pak_writer::official_pak(&[
            (
                "Items/D_ItemsStatic.json",
                pak_writer::table_json(&["Wood", "Stone", "Tonic"]),
            ),
            (
                "Flags/D_SessionFlags.json",
                pak_writer::table_json(&["Flag_A"]),
            ),
        ]),
    )
    .unwrap();
    icarus_bevy::data_pak::DataTables::open(&pak).unwrap()
}

/// The saved container manager (`C016`) types, and with the server's data
/// tables maps to inventories: each info row's slots with the saved items
/// in place, one more replication-key change per item (F-866); a list with
/// empty positions or an item with fields the server does not send yet
/// skips the manager.
#[test]
fn saved_containers_map_to_inventories() {
    use icarus_bevy::experimental::control_messages::ArchiveText;
    use icarus_bevy::experimental::manager_open::ManagerSlot;
    use icarus_bevy::experimental::pawn_open::{InventorySlot, SlotItem};
    let inner = payload(&prospect_info(), &[two_containers()]);
    let saved = saved_prospect_from_payload(&inner)
        .unwrap()
        .inventory_containers
        .expect("a container manager record");
    assert_eq!(saved.count, 2);
    assert_eq!(saved.containers.len(), 2);
    let item = &saved.containers[0].items[0];
    assert_eq!(
        (item.location, &item.row_name, &item.dynamic),
        (1, &units("Tonic"), &vec![(7, 4)])
    );
    assert_eq!(item.stats, [(-1, units("Potency"), 3)]);
    let root = tempfile::TempDir::new().unwrap();
    let mut tables = container_tables(root.path());
    let inventories = icarus_bevy::network::container_inventories(&mut tables, &saved).unwrap();
    assert_eq!(inventories.len(), 2);
    // Hands is D_InventoryInfo row 1: slots query Any, Any, Fists.
    assert_eq!(inventories[0].info_row, 1);
    assert_eq!(inventories[0].slots.replication_key, 3);
    assert_eq!(
        inventories[0].slots.items,
        [
            InventorySlot::empty(1, 0),
            InventorySlot {
                item: Some(SlotItem {
                    row: 2,
                    dynamic: vec![(7, 4)],
                    stats: vec![(ArchiveText::new(&units("Potency")).unwrap(), 3)],
                }),
                ..InventorySlot::empty(1, 1)
            },
            InventorySlot::empty(2, 2),
        ]
    );
    assert_eq!(inventories[1].info_row, 2);
    assert!(inventories[1].slots.items.is_empty());
    assert_eq!(inventories[1].slots.replication_key, 2);
    let section = PreservedSection::from_inner_payload(&inner, PreserveLimits::default()).unwrap();
    let state = icarus_bevy::network::saved_game_state(&section, Some(&mut tables)).unwrap();
    assert_eq!(state.inventory_containers, Some(inventories));
    assert!(state.skipped_managers.is_empty());
    // Saves the server cannot send yet skip the manager, with the reason.
    for bad in [
        container_manager_recorder(3, &[(0, "Hands", Vec::new()), (2, "Bag", Vec::new())]),
        container_manager_recorder(
            1,
            &[(
                0,
                "Hands",
                vec![saved_item(
                    0,
                    &[struct_array(
                        "Alterations",
                        "InventorySlotAlterationData",
                        &[end()],
                    )],
                )],
            )],
        ),
        container_manager_recorder(1, &[(0, "Hands", vec![saved_item(3, &[])])]),
        container_manager_recorder(1, &[(0, "Missing", Vec::new())]),
        container_manager_recorder(
            1,
            &[(0, "Hands", vec![saved_item(0, &[]), saved_item(0, &[])])],
        ),
        container_manager_recorder(
            1,
            &[(
                0,
                "Hands",
                vec![item_with(
                    &ItemFields {
                        guid: "g",
                        ..ItemFields::at(0)
                    },
                    &[],
                )],
            )],
        ),
        container_manager_recorder(
            1,
            &[(
                0,
                "Hands",
                vec![item_with(
                    &ItemFields {
                        owner: 5,
                        ..ItemFields::at(0)
                    },
                    &[],
                )],
            )],
        ),
        container_manager_recorder(
            1,
            &[(
                0,
                "Hands",
                vec![item_with(
                    &ItemFields {
                        stat_index: 2,
                        ..ItemFields::at(0)
                    },
                    &[],
                )],
            )],
        ),
        container_manager_recorder(
            1,
            &[(
                0,
                "Hands",
                vec![item_with(
                    &ItemFields {
                        dynamic_property: 16,
                        ..ItemFields::at(0)
                    },
                    &[],
                )],
            )],
        ),
    ] {
        let section = PreservedSection::from_inner_payload(
            &payload(&prospect_info(), &[bad]),
            PreserveLimits::default(),
        )
        .unwrap();
        let state = icarus_bevy::network::saved_game_state(&section, Some(&mut tables)).unwrap();
        assert!(state.inventory_containers.is_none());
        assert_eq!(state.skipped_managers.len(), 1);
        assert_eq!(state.skipped_managers[0].0, ManagerSlot::InventoryContainer);
    }
}

/// The saved player history (`C024`) types in saved order (F-862).
#[test]
fn a_saved_player_history_supplies_the_history() {
    use icarus_bevy::experimental::prospect_info::{PlayerHistoryEntry, saved_player_history};
    let inner = payload(&prospect_info(), &[player_history_recorder()]);
    let history = saved_prospect_from_payload(&inner)
        .unwrap()
        .player_history
        .expect("a player history record");
    let expected: Vec<PlayerHistoryEntry> = HISTORY
        .iter()
        .map(|(user, slot, name)| PlayerHistoryEntry {
            user_id: units(user),
            chr_slot: *slot,
            cached_character_name: units(name),
        })
        .collect();
    assert_eq!(history, expected);
    let section = PreservedSection::from_inner_payload(&inner, PreserveLimits::default()).unwrap();
    let saved = icarus_bevy::network::saved_game_state(&section, None).unwrap();
    assert_eq!(saved.player_history, Some(expected));
    // Two history recorders are rejected; mistyped fields fail closed.
    assert!(
        saved_prospect_from_payload(&payload(
            &prospect_info(),
            &[player_history_recorder(), player_history_recorder()],
        ))
        .is_err()
    );
    let typed = |fields: &[Vec<u8>]| {
        let stream = [fields.concat(), end()].concat();
        let tree =
            PropertyTree::parse(&stream, TagProfile::MemoryArchive522, TreeLimits::default())
                .unwrap();
        saved_player_history(&TypedTree::from_tree(&tree).unwrap().0)
    };
    assert!(typed(&[]).unwrap().is_empty());
    let entry = |field: Vec<u8>| {
        struct_array(
            "SavedHistoryData",
            "PlayerHistoryEntryRecord",
            &[[field, end()].concat()],
        )
    };
    assert_eq!(typed(&[entry(int("ChrSlot", 2))]).unwrap()[0].chr_slot, 2);
    for bad in [
        int("SavedHistoryData", 1),
        entry(string("ChrSlot", "2")),
        entry(int("UserID", 7)),
    ] {
        assert!(typed(&[bad]).is_err());
    }
}

/// The saved quest manager (`C018`) types from its record, and with the
/// server's data tables the faction mission maps to its row (F-861).
#[test]
fn a_saved_quest_manager_supplies_the_quest_manager_state() {
    use icarus_bevy::experimental::prospect_info::RelevantQuestActor;
    let inner = payload(
        &prospect_info(),
        &[game_mode_recorder(150), quest_manager_recorder()],
    );
    let saved = saved_prospect_from_payload(&inner).unwrap();
    let quest = saved.quest_manager.expect("a quest manager record");
    assert_eq!(quest.faction_mission, units("Mission_C"));
    assert_eq!(quest.initial_quest, Some(9_001));
    assert!(quest.run_quests && !quest.mission_complete);
    assert_eq!(quest.dynamic_quest_difficulty, 2);
    assert_eq!(quest.dynamic_quest_delay, 0.0);
    assert_eq!(
        quest.persistent_actors,
        [
            RelevantQuestActor {
                key: units("LAB_BASE"),
                icarus_uid: 501,
            },
            RelevantQuestActor {
                key: units("LAB_GONE"),
                icarus_uid: -1,
            },
        ]
    );
    assert!(quest.persistent_characters.is_empty());
    let section = PreservedSection::from_inner_payload(&inner, PreserveLimits::default()).unwrap();
    // Without the server's data tables the faction mission has no row.
    let saved = icarus_bevy::network::saved_game_state(&section, None).unwrap();
    assert!(saved.quest_manager.is_none());
    let root = tempfile::TempDir::new().unwrap();
    let pak = root.path().join("data.pak");
    std::fs::write(
        &pak,
        pak_writer::official_pak(&[
            (
                "Flags/D_SessionFlags.json",
                pak_writer::table_json(&["Flag_A"]),
            ),
            ("Factions/D_FactionMissions.json", faction_missions_json()),
        ]),
    )
    .unwrap();
    let mut tables = icarus_bevy::data_pak::DataTables::open(&pak).unwrap();
    let saved = icarus_bevy::network::saved_game_state(&section, Some(&mut tables)).unwrap();
    let (saved_quest, row) = saved.quest_manager.expect("a mapped quest manager");
    assert_eq!(row, 3);
    assert_eq!(saved_quest, quest);
    assert!(saved.skipped_managers.is_empty());
    // A faction mission the tables lack skips the QuestManager, with a
    // reason.
    let other = root.path().join("other.pak");
    std::fs::write(
        &other,
        pak_writer::official_pak(&[
            (
                "Flags/D_SessionFlags.json",
                pak_writer::table_json(&["Flag_A"]),
            ),
            (
                "Factions/D_FactionMissions.json",
                pak_writer::table_json(&["Mission_Z"]),
            ),
        ]),
    )
    .unwrap();
    let mut other = icarus_bevy::data_pak::DataTables::open(&other).unwrap();
    let saved = icarus_bevy::network::saved_game_state(&section, Some(&mut other)).unwrap();
    assert!(saved.quest_manager.is_none());
    assert_eq!(
        saved
            .skipped_managers
            .iter()
            .map(|(slot, _)| *slot)
            .collect::<Vec<_>>(),
        [icarus_bevy::experimental::manager_open::ManagerSlot::Quest]
    );
    // Without the record, nothing to open.
    let bare = PreservedSection::from_inner_payload(
        &payload(&prospect_info(), &[game_mode_recorder(150)]),
        PreserveLimits::default(),
    )
    .unwrap();
    let saved = icarus_bevy::network::saved_game_state(&bare, Some(&mut tables)).unwrap();
    assert!(saved.quest_manager.is_none());
}

#[test]
fn mistyped_quest_manager_fields_fail_closed() {
    use icarus_bevy::experimental::prospect_info::SavedQuestManager;
    let typed = |fields: &[Vec<u8>]| {
        let stream = [fields.concat(), end()].concat();
        let tree =
            PropertyTree::parse(&stream, TagProfile::MemoryArchive522, TreeLimits::default())
                .unwrap();
        SavedQuestManager::from_tree(&TypedTree::from_tree(&tree).unwrap().0)
    };
    assert_eq!(typed(&[]).unwrap(), SavedQuestManager::default());
    // A non-positive initial quest GUID links nothing.
    let initial = |uid| {
        struct_tag(
            "InitialQuestRecord",
            "InitialQuestRecord",
            &[int("RelevantActorIcarusUID", uid), end()].concat(),
        )
    };
    assert_eq!(typed(&[initial(0)]).unwrap().initial_quest, None);
    assert_eq!(typed(&[initial(7)]).unwrap().initial_quest, Some(7));
    for bad in [
        string("FactionMissionName", "Mission_C"),
        int("DynamicQuestDifficulty", 256),
        string("DynamicQuestDifficulty", "2"),
        int("bRunQuests", 1),
        int("RelevantPersistentActorRecords", 1),
        struct_array(
            "RelevantPersistentActorRecords",
            "RelevantQuestActorRecord",
            &[[string("RelevantActorIcarusUID", "5"), end()].concat()],
        ),
    ] {
        assert!(typed(&[bad]).is_err());
    }
}

/// The GameState's open-world and outpost flags follow the prospect row's
/// `WorldStatList`: each is set when its `D_ProspectStats` world stat sums
/// above 0 (F-911); a prospect the table lacks sets neither.
#[test]
fn prospect_rows_set_the_open_world_and_outpost_flags() {
    use icarus_bevy::network::prospect_world_flags;
    let root = tempfile::TempDir::new().unwrap();
    let pak = root.path().join("data.pak");
    let list = json!({"Rows": [
        {"Name": "Lab_Key", "WorldStatList": [
            {"RowName": "OpenWorld", "DataTableName": "D_ProspectStats"},
            {"RowName": "Plain", "DataTableName": "D_ProspectStats"},
        ]},
        {"Name": "Lab_Outpost", "WorldStatList": [
            {"RowName": "Outpost", "DataTableName": "D_ProspectStats"},
            {"RowName": "Cancel", "DataTableName": "D_ProspectStats"},
        ]},
        {"Name": "Lab_None"},
        {"Name": "Lab_Bad", "WorldStatList": [{"Table": "D_ProspectStats"}]},
    ]});
    let stats = json!({"Rows": [
        {"Name": "OpenWorld", "WorldStats": {"(Value=\"WorldIsOpenWorldProspect_?\")": 1}},
        {"Name": "Outpost", "WorldStats": {"(Value=\"WorldIsOutpostProspect_?\")": 1}},
        {"Name": "Cancel", "WorldStats": {"(Value=\"WorldIsOutpostProspect_?\")": -1}},
        {"Name": "Plain", "WorldStats": {"(Value=\"OtherStat_+\")": 5}},
    ]});
    std::fs::write(
        &pak,
        pak_writer::official_pak(&[
            (
                "Prospects/D_ProspectList.json",
                serde_json::to_vec(&list).unwrap(),
            ),
            (
                "Stats/D_ProspectStats.json",
                serde_json::to_vec(&stats).unwrap(),
            ),
            (
                "Flags/D_SessionFlags.json",
                pak_writer::table_json(&["Flag_A"]),
            ),
            ("Factions/D_FactionMissions.json", faction_missions_json()),
        ]),
    )
    .unwrap();
    let mut tables = icarus_bevy::data_pak::DataTables::open(&pak).unwrap();
    assert_eq!(
        prospect_world_flags(&mut tables, &units("Lab_Key")).unwrap(),
        [true, false]
    );
    // An outpost grant cancelled by another row sums to 0.
    assert_eq!(
        prospect_world_flags(&mut tables, &units("Lab_Outpost")).unwrap(),
        [false, false]
    );
    for none in ["Lab_None", "Missing"] {
        assert_eq!(
            prospect_world_flags(&mut tables, &units(none)).unwrap(),
            [false, false]
        );
    }
    assert!(prospect_world_flags(&mut tables, &units("Lab_Bad")).is_err());
    // The saved prospect (`Lab_Key`) sends `bIsOpenWorldProspect` only.
    let section = PreservedSection::from_inner_payload(
        &payload(&prospect_info(), &[game_mode_recorder(150)]),
        PreserveLimits::default(),
    )
    .unwrap();
    let saved = icarus_bevy::network::saved_game_state(&section, Some(&mut tables)).unwrap();
    assert_eq!(
        saved.values.get(&handle::IS_OPEN_WORLD_PROSPECT),
        Some(&RepValue::Bool(true))
    );
    assert!(!saved.values.contains_key(&handle::IS_OUTPOST_PROSPECT));
}

/// A child process killed and reaped when dropped, also when a test panics.
struct Killed(std::process::Child);

impl Drop for Killed {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

/// `run` on a world preserved from a save sends the save's active prospect,
/// game mode values, advancing level clock and time of day in its GameState
/// open (real binary, authored probe on loopback).
#[test]
fn run_replicates_a_preserved_worlds_prospect() {
    use icarus_bevy::experimental::{
        actor_channel::{ActorChannelCodec, ActorChannelLimits, read_actor_bunch_body},
        archive::ByteOrder,
        control_messages::{ControlCodec, ControlMessage, NetIdType, NetIdValue, UniqueNetId},
        packet::{PacketConfig, Receiver, read_packet},
        peer_lab::{PeerLabAdmission, PeerLabConfig},
        probe_client::{ProbeClient, ProbeConfig},
        release::ReleaseProfile,
        rep_layout::{RepLayoutCodec, RepLayoutLimits, read_rep_properties},
    };
    use std::{
        io::{BufRead, BufReader},
        net::{SocketAddr, UdpSocket},
        process::{Command, Stdio},
        time::{Duration, Instant},
    };

    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("prospect.json");
    std::fs::write(
        &source,
        envelope(&payload(
            &prospect_info(),
            &[
                game_mode_recorder(150),
                quest_manager_recorder(),
                player_history_recorder(),
                two_containers(),
            ],
        )),
    )
    .unwrap();
    let state = directory.path().join("state");
    let preserved = Command::new(env!("CARGO_BIN_EXE_icarus-server"))
        .arg("preserve-save")
        .arg("--source")
        .arg(&source)
        .arg("--data-dir")
        .arg(&state)
        .output()
        .unwrap();
    assert!(preserved.status.success(), "{preserved:?}");
    let release = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml");
    let profile = ReleaseProfile::load(&release).unwrap();
    // The official server's data tables, authored, where the install step
    // puts them in the data directory: two of the three saved session flags
    // are listed, and run finds them without a configured data_pak.
    let pak = state
        .join(icarus_bevy::network::OFFICIAL_DIR)
        .join(icarus_bevy::network::OFFICIAL_DATA_PAK);
    std::fs::create_dir_all(pak.parent().unwrap()).unwrap();
    std::fs::write(
        &pak,
        pak_writer::official_pak(&[
            (
                "Flags/D_SessionFlags.json",
                pak_writer::table_json(&["Flag_A", "Flag_X", "Flag_B"]),
            ),
            ("Factions/D_FactionMissions.json", faction_missions_json()),
            (
                "Items/D_ItemsStatic.json",
                pak_writer::table_json(&["Wood", "Stone", "Tonic"]),
            ),
        ]),
    )
    .unwrap();
    let config = directory.path().join("server.toml");
    std::fs::write(
        &config,
        format!(
            "[network]\nenabled = true\nport = 0\nrelease_profile = {}\n",
            toml::Value::String(release.display().to_string())
        ),
    )
    .unwrap();
    let mut child = Killed(
        Command::new(env!("CARGO_BIN_EXE_icarus-server"))
            .arg("run")
            .arg("--config")
            .arg(&config)
            .args(["--tick-hz", "240", "--max-ticks", "1000000", "--data-dir"])
            .arg(&state)
            .stderr(Stdio::piped())
            .stdout(Stdio::null())
            .spawn()
            .unwrap(),
    );
    // Read records on a thread so the deadline holds while none arrive.
    let stderr = child.0.stderr.take().unwrap();
    let (sender, records) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines() {
            if sender.send(line).is_err() {
                break;
            }
        }
    });
    let deadline = Instant::now() + Duration::from_secs(20);
    let mut replicated = false;
    let address: SocketAddr = loop {
        let line = records
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("a network_listening record within 20 s")
            .unwrap();
        let record: serde_json::Value = serde_json::from_str(&line).unwrap();
        if record["event"] == "active_prospect_replicated" {
            replicated = true;
            assert_eq!(record["unmapped_session_flags"], 1);
            assert_eq!(record["locked_missions"], 2);
            // The saved quest manager opens after the other managers.
            assert_eq!(record["quest_manager_channel"], 9);
            assert_eq!(record["container_manager_channel"], 10);
            assert_eq!(record["player_history_channel"], 11);
            assert_eq!(record["skipped_managers"], json!([]));
        }
        if record["event"] == "network_listening" {
            // The installed official data tables were picked up.
            assert_eq!(record["data_pak_source"], "installed");
            break record["address"].as_str().unwrap().parse().unwrap();
        }
    };
    assert!(replicated, "the preserved prospect replicates");
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(address).unwrap();
    let mut client = ProbeClient::connect(
        socket,
        ProbeConfig {
            packet: PacketConfig {
                magic_bits: 0,
                receiver: Receiver::Server,
                limits: PeerLabConfig::default().packet.packet.limits,
            },
            codec: ControlCodec::default(),
            handshake_timeout: Duration::from_secs(2),
            handshake_attempts: 3,
            reply_window: Duration::from_secs(3),
            max_datagrams: 1,
        },
    )
    .unwrap();
    client.hello(profile.pin.network_checksum, "").unwrap();
    client
        .send_control(
            &[ControlMessage::Login {
                response: ArchiveText::text("0").unwrap(),
                url: ArchiveText::text("?Name=Probe").unwrap(),
                unique_net_id: UniqueNetId {
                    kind: NetIdType::Named(ArchiveText::text("STEAM").unwrap()),
                    value: NetIdValue::from_hex("0123456789ABCDEF7654").unwrap(),
                },
                platform: ArchiveText::text("").unwrap(),
            }],
            false,
        )
        .unwrap();
    client.collect(Duration::from_secs(3), 1).unwrap();
    client
        .send_control(
            &[
                ControlMessage::Netspeed { rate: 100_000 },
                ControlMessage::Join,
            ],
            false,
        )
        .unwrap();
    let collected = client.collect(Duration::from_secs(3), 16).unwrap();
    drop(child);
    let to_client = PacketConfig {
        magic_bits: 0,
        receiver: Receiver::Client,
        limits: PeerLabConfig::default().packet.packet.limits,
    };
    let body = collected
        .replies
        .iter()
        .flat_map(|reply| read_packet(&reply.bytes, &to_client).unwrap().bunches)
        .find(|bunch| {
            bunch.header.channel_index
                == icarus_bevy::experimental::game_state_open::GAME_STATE_CHANNEL_INDEX
                && !bunch.header.has_package_map_exports
        })
        .expect("a GameState open");
    let codec = ActorChannelCodec {
        engine_net_version: profile.engine_network_version,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    };
    let plan = profile
        .game_state_open_plan(PeerLabAdmission::default().map_url)
        .unwrap()
        .unwrap();
    let open = read_actor_bunch_body(&mut body.payload.reader(), true, &codec).unwrap();
    let stream = read_rep_properties(
        &mut open.blocks[0].payload.reader(),
        &plan.layout,
        &RepLayoutCodec {
            actor: codec,
            checksums: false,
            limits: RepLayoutLimits::default(),
        },
    )
    .unwrap();
    let mut sent: BTreeMap<u32, RepValue> = stream
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect();
    // The level clock: restored plus the whole world seconds at the send.
    let Some(&RepValue::Float(world_time)) = sent.get(&handle::WORLD_TIME_SECONDS) else {
        panic!("world time");
    };
    assert_eq!(
        sent.remove(&handle::LEVEL_TIME_ELAPSED_SEC),
        Some(RepValue::Int(level_time_elapsed_sec(
            150,
            f32::from_bits(world_time)
        )))
    );
    // TimeOfDay: the saved 12.5 minutes advanced at 1440 / 3600 minutes
    // per world second since the clock started, at most the world time.
    let Some(RepValue::Float(time_of_day)) = sent.remove(&handle::TIME_OF_DAY) else {
        panic!("time of day");
    };
    let time_of_day = f32::from_bits(time_of_day);
    let latest = 12.5 + f32::from_bits(world_time) * 1_440.0 / 3_600.0;
    assert!(
        (12.5..=latest + 1e-3).contains(&time_of_day),
        "{time_of_day} outside 12.5..={latest}"
    );
    let mut expected = expected_saved_values();
    expected.remove(&handle::TIME_OF_DAY);
    expected.insert(
        handle::SESSION_FLAGS,
        icarus_bevy::experimental::game_state_open::session_flags_value(&[2, 0]),
    );
    // LockedMissions as refreshed at the restored level clock (the open
    // went out within the first minute of world time).
    assert!(f32::from_bits(world_time) < 60.0);
    expected.insert(
        handle::LOCKED_MISSIONS,
        locked_missions_value(&expected_locked_missions(), 150).unwrap(),
    );
    sent.retain(|handle, _| expected.contains_key(handle));
    assert_eq!(sent, expected);
    // The QuestManager open carries the saved quest state (F-861).
    let quest = collected
        .replies
        .iter()
        .flat_map(|reply| read_packet(&reply.bytes, &to_client).unwrap().bunches)
        .find(|bunch| bunch.header.channel_index == 9 && !bunch.header.has_package_map_exports)
        .expect("a QuestManager open");
    let open = read_actor_bunch_body(&mut quest.payload.reader(), true, &codec).unwrap();
    let plan = profile
        .manager_open_plan(
            icarus_bevy::experimental::manager_open::ManagerSlot::Quest,
            PeerLabAdmission::default().map_url,
        )
        .unwrap()
        .unwrap();
    let stream = read_rep_properties(
        &mut open.blocks[0].payload.reader(),
        &plan.layout,
        &RepLayoutCodec {
            actor: codec,
            checksums: false,
            limits: RepLayoutLimits::default(),
        },
    )
    .unwrap();
    let quest: BTreeMap<u32, RepValue> = stream
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect();
    assert_eq!(quest.get(&19), Some(&RepValue::Byte(3)));
    assert_eq!(quest.get(&20), Some(&RepValue::Enum(2)));
    assert_eq!(quest.get(&24), Some(&RepValue::Bool(true)));
    assert!(quest.contains_key(&21) && quest.contains_key(&25));
    // The PlayerHistoryTracker open carries the saved history (F-862).
    let history = collected
        .replies
        .iter()
        .flat_map(|reply| read_packet(&reply.bytes, &to_client).unwrap().bunches)
        .find(|bunch| bunch.header.channel_index == 11 && !bunch.header.has_package_map_exports)
        .expect("a PlayerHistoryTracker open");
    let open = read_actor_bunch_body(&mut history.payload.reader(), true, &codec).unwrap();
    let plan = profile
        .manager_open_plan(
            icarus_bevy::experimental::manager_open::ManagerSlot::PlayerHistory,
            PeerLabAdmission::default().map_url,
        )
        .unwrap()
        .unwrap();
    let stream = read_rep_properties(
        &mut open.blocks[0].payload.reader(),
        &plan.layout,
        &RepLayoutCodec {
            actor: codec,
            checksums: false,
            limits: RepLayoutLimits::default(),
        },
    )
    .unwrap();
    let Some(RepValue::DynamicArray(sent)) = stream
        .entries
        .into_iter()
        .find_map(|entry| (entry.handle == 19).then_some(entry.value))
    else {
        panic!("a History array");
    };
    assert_eq!(sent.count, 2);
    assert_eq!(sent.stream.entries.len(), 6);
    assert_eq!(sent.stream.entries[4].value, RepValue::Int(3));
    assert_eq!(sent.stream.entries[5].value, text("Bjørn"));
    // The InventoryContainerManager open lists its two saved inventories
    // and opens them as runtime subobjects (F-866).
    let containers = collected
        .replies
        .iter()
        .flat_map(|reply| read_packet(&reply.bytes, &to_client).unwrap().bunches)
        .find(|bunch| bunch.header.channel_index == 10 && !bunch.header.has_package_map_exports)
        .expect("an InventoryContainerManager open");
    let open = read_actor_bunch_body(&mut containers.payload.reader(), true, &codec).unwrap();
    assert_eq!(open.blocks.len(), 3);
    let plan = profile
        .manager_open_plan(
            icarus_bevy::experimental::manager_open::ManagerSlot::InventoryContainer,
            PeerLabAdmission::default().map_url,
        )
        .unwrap()
        .unwrap();
    let stream = read_rep_properties(
        &mut open.blocks[0].payload.reader(),
        &plan.layout,
        &RepLayoutCodec {
            actor: codec,
            checksums: false,
            limits: RepLayoutLimits::default(),
        },
    )
    .unwrap();
    let Some(RepValue::DynamicArray(listed)) = stream
        .entries
        .into_iter()
        .find_map(|entry| (entry.handle == 19).then_some(entry.value))
    else {
        panic!("an Inventories array");
    };
    assert_eq!(listed.count, 2);
}

#[test]
fn mistyped_game_mode_fields_fail_closed() {
    let typed = |fields: &[Vec<u8>]| {
        let stream = [fields.concat(), end()].concat();
        let tree =
            PropertyTree::parse(&stream, TagProfile::MemoryArchive522, TreeLimits::default())
                .unwrap();
        SavedGameMode::from_tree(&TypedTree::from_tree(&tree).unwrap().0)
    };
    assert_eq!(typed(&[]).unwrap(), SavedGameMode::default());
    let record =
        |field: Vec<u8>| struct_tag("GameModeRecord", "GameModeRecord", &[field, end()].concat());
    assert_eq!(
        typed(&[record(int("SecondsPerGameDay", 5))])
            .unwrap()
            .seconds_per_game_day,
        5
    );
    for bad in [
        record(int("TimeOfDay", 5)),
        record(tag(
            "GameStateSeed",
            "FloatProperty",
            &[0],
            &1.0_f32.to_le_bytes(),
        )),
        int("GameModeRecord", 1),
        string("DynamicQuestSeed", "7"),
    ] {
        assert!(typed(&[bad]).is_err());
    }
}
