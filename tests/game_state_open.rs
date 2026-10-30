use std::collections::BTreeMap;

use icarus_bevy::experimental::actor_channel::{
    ActorChannelCodec, ActorChannelLimits, BlockTarget, PackageMapState, read_actor_bunch_body,
};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::game_state_open::{
    GameStateComponents, GameStateManagers, GameStateOpenPlan, GameStateOpenState,
    compose_game_state_open, compose_game_state_update, handle,
};
use icarus_bevy::experimental::rep_layout::{
    MovementLevel, RepEntry, RepLayoutCodec, RepLayoutLimits, RepValue, RotationLevel,
    icarus_game_state_survival_rep_layout, read_rep_properties,
    weather_manager_component_rep_layout,
};
use icarus_bevy::experimental::session_open::{
    ActorKey, ComponentBlock, ComponentExport, ConnectionExports, NetGuidCache, PathSegment,
};

fn codec() -> ActorChannelCodec {
    ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    }
}

fn rep_codec() -> RepLayoutCodec {
    RepLayoutCodec {
        actor: codec(),
        checksums: false,
        limits: RepLayoutLimits::default(),
    }
}

fn component(name: &str, checksum: u32) -> ComponentExport {
    ComponentExport {
        name: name.to_owned(),
        checksum,
        block: ComponentBlock::None,
    }
}

/// Authored identifiers in the shape of F-794 (not release data).
fn plan() -> GameStateOpenPlan {
    GameStateOpenPlan {
        archetype: vec![
            PathSegment::new("Default__TestGameState_C", 0x1111_0001, false),
            PathSegment::new("/Game/Test/TestGameState", 0, false),
        ],
        level: vec![
            PathSegment::new("PersistentLevel", 0, true),
            PathSegment::new("TestMap", 0, true),
            PathSegment::new("/Game/Maps/TestMap", 0, true),
        ],
        game_mode_class: vec![
            PathSegment::new("TestGameMode_C", 0x1111_0002, false),
            PathSegment::new("/Game/Test/TestGameMode", 0, false),
        ],
        spectator_class: vec![
            PathSegment::new("TestSpectator", 0x1111_0003, false),
            PathSegment::new("/Script/Test", 0, false),
        ],
        components: GameStateComponents {
            connected_players: component("Players", 0x2222_0001),
            dialogue_system: component("Dialogue", 0x2222_0002),
            weather: component("Weather", 0x2222_0003),
        },
        layout: icarus_game_state_survival_rep_layout(
            MovementLevel::Whole,
            MovementLevel::Whole,
            RotationLevel::Bytes,
        ),
        weather_layout: weather_manager_component_rep_layout(),
        engine_net_version: 17,
    }
}

const GAME_STATE: ActorKey = ActorKey::Session(0);
const FLOD: ActorKey = ActorKey::World(901);
const QUESTS: ActorKey = ActorKey::World(902);

fn state() -> GameStateOpenState {
    GameStateOpenState {
        key: GAME_STATE,
        managers: GameStateManagers {
            quest_manager: Some(QUESTS),
            flod: Some(FLOD),
            ..GameStateManagers::default()
        },
        values: BTreeMap::from([
            (handle::HAS_BEGUN_PLAY, RepValue::Bool(true)),
            (
                handle::WORLD_TIME_SECONDS,
                RepValue::Float(12.5f32.to_bits()),
            ),
            (handle::TIME_OF_DAY, RepValue::Float(600.0f32.to_bits())),
            (handle::SERVER_FPS, RepValue::Int(30)),
        ]),
        weather: Vec::new(),
    }
}

/// Decoded export GUIDs of a composed open, in prefix order.
fn exported_guids(open: &icarus_bevy::experimental::session_open::ComposedActorOpen) -> Vec<u32> {
    let Some(exports) = &open.exports else {
        return Vec::new();
    };
    PackageMapState::new()
        .consume_export_prefix(&mut exports.reader(), &codec())
        .unwrap()
        .iter()
        .map(|reference| reference.guid)
        .collect()
}

#[test]
fn game_state_open_composes_references_in_the_captured_order() {
    let plan = plan();
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    // The level went out in packet 1, which the client acked (F-796).
    cache.export_static(&mut exports, &plan.level).unwrap();
    exports.commit_bunch(1);
    exports.received_ack(1);
    let open = compose_game_state_open(
        &mut cache,
        &mut exports,
        &plan,
        &state(),
        4,
        ByteOrder::Little,
    )
    .unwrap();
    // GameState 2; Players 4 and Dialogue 6 by property; managers by handle
    // order (QuestManager 8, FLOD 10); Weather last (12).
    assert_eq!(cache.actor_guid(GAME_STATE).unwrap(), 2);
    assert_eq!(cache.actor_guid(QUESTS).unwrap(), 8);
    assert_eq!(cache.actor_guid(FLOD).unwrap(), 10);
    // Archetype, game mode and spectator chains (the level was exported),
    // then the three components.
    let statics: Vec<u32> = exported_guids(&open);
    assert_eq!(statics.len(), 6);
    assert_eq!(&statics[3..], [4, 6, 12]);
    let header = &open.bunch.header;
    assert!(header.open && header.reliable && header.channel_index == 4);
    let body = read_actor_bunch_body(&mut open.bunch.payload.reader(), true, &codec()).unwrap();
    let spawn = body.spawn.unwrap();
    assert_eq!(spawn.actor.guid, 2);
    let dynamic = spawn.dynamic.unwrap();
    assert!(dynamic.location.is_none() && dynamic.rotation.is_none());
    assert_eq!(body.blocks.len(), 4);
    let stream = read_rep_properties(
        &mut body.blocks[0].payload.reader(),
        &plan.layout,
        &rep_codec(),
    )
    .unwrap();
    let values: BTreeMap<u32, RepValue> = stream
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect();
    let handles: Vec<u32> = values.keys().copied().collect();
    assert_eq!(handles, [5, 14, 16, 17, 18, 19, 22, 23, 25, 36, 38, 82]);
    let object = |handle| match &values[&handle] {
        RepValue::Object(reference) => reference.guid,
        other => panic!("handle {handle} is {other:?}"),
    };
    assert_eq!(object(handle::GAME_MODE_CLASS), statics[1]);
    assert_eq!(object(handle::SPECTATOR_CLASS), statics[2]);
    assert_eq!(object(handle::CONNECTED_PLAYERS), 4);
    assert_eq!(object(handle::DIALOGUE_SYSTEM), 6);
    assert_eq!(object(handle::QUEST_MANAGER), 8);
    assert_eq!(object(handle::FLOD), 10);
    assert!(matches!(values[&handle::REMOTE_ROLE], RepValue::Byte(1)));
    assert!(matches!(values[&handle::ROLE], RepValue::Byte(3)));
    // Component blocks: stably named, the weather one without properties.
    for (block, guid) in body.blocks[1..].iter().zip([4, 6, 12]) {
        assert!(!block.has_rep_layout && block.payload.bit_len() == 0);
        assert!(matches!(
            &block.target,
            BlockTarget::SubObject { object, .. } if object.guid == guid
        ));
    }
}

#[test]
fn every_connection_gets_the_same_guids_with_its_own_exports() {
    let plan = plan();
    let mut cache = NetGuidCache::new();
    let mut first = ConnectionExports::new();
    let one = compose_game_state_open(
        &mut cache,
        &mut first,
        &plan,
        &state(),
        4,
        ByteOrder::Little,
    )
    .unwrap();
    let mut second = ConnectionExports::new();
    let two = compose_game_state_open(
        &mut cache,
        &mut second,
        &plan,
        &state(),
        5,
        ByteOrder::Little,
    )
    .unwrap();
    // The second connection receives the same objects and paths, the level
    // included; the body differs only in its channel header.
    assert_eq!(exported_guids(&one), exported_guids(&two));
    assert_eq!(one.exports, two.exports);
    assert_eq!(one.bunch.payload, two.bunch.payload);
    // A manager's own open later reuses the GUID the GameState named.
    assert_eq!(cache.actor_guid(FLOD).unwrap(), 10);
    // Until a packet carrying them is acked, every open exports the paths
    // again (F-796); afterwards it exports nothing.
    let mut reopen = |exports: &mut ConnectionExports| {
        compose_game_state_open(&mut cache, exports, &plan, &state(), 6, ByteOrder::Little).unwrap()
    };
    first.commit_bunch(1);
    let pending = reopen(&mut first);
    assert_eq!(pending.exports, one.exports);
    first.commit_bunch(2);
    first.received_ack(1);
    let again = reopen(&mut first);
    assert!(again.exports.is_none());
    assert_eq!(again.bunch.payload, one.bunch.payload);
}

#[test]
fn a_manager_opened_first_keeps_its_guid_in_the_game_state() {
    let plan = plan();
    let mut cache = NetGuidCache::new();
    let flod = cache.actor_guid(FLOD).unwrap();
    let mut exports = ConnectionExports::new();
    let open = compose_game_state_open(
        &mut cache,
        &mut exports,
        &plan,
        &state(),
        4,
        ByteOrder::Little,
    )
    .unwrap();
    let body = read_actor_bunch_body(&mut open.bunch.payload.reader(), true, &codec()).unwrap();
    let stream = read_rep_properties(
        &mut body.blocks[0].payload.reader(),
        &plan.layout,
        &rep_codec(),
    )
    .unwrap();
    let named = stream
        .entries
        .iter()
        .find(|entry| entry.handle == handle::FLOD)
        .unwrap();
    assert!(matches!(&named.value, RepValue::Object(reference) if reference.guid == flod));
}

#[test]
fn session_and_world_keys_never_share_a_guid() {
    let mut cache = NetGuidCache::new();
    let session = cache.actor_guid(ActorKey::Session(7)).unwrap();
    let world = cache.actor_guid(ActorKey::World(7)).unwrap();
    assert_ne!(session, world);
    assert_eq!(cache.actor_guid(ActorKey::Session(7)).unwrap(), session);
    assert_eq!(cache.actor_guid(ActorKey::World(7)).unwrap(), world);
}

#[test]
fn weather_properties_open_the_component_with_its_layout() {
    let plan = plan();
    let mut state = state();
    // `UActorComponent` handle 2 (`bIsActive`, F-703).
    state.weather = vec![RepEntry {
        handle: 2,
        checksum: None,
        value: RepValue::Bool(true),
    }];
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    let open = compose_game_state_open(
        &mut cache,
        &mut exports,
        &plan,
        &state,
        4,
        ByteOrder::Little,
    )
    .unwrap();
    let body = read_actor_bunch_body(&mut open.bunch.payload.reader(), true, &codec()).unwrap();
    let weather = &body.blocks[3];
    assert!(weather.has_rep_layout);
    let stream = read_rep_properties(
        &mut weather.payload.reader(),
        &plan.weather_layout,
        &rep_codec(),
    )
    .unwrap();
    assert_eq!(stream.entries, state.weather);
}

#[test]
fn composed_skipped_and_out_of_range_handles_are_rejected() {
    let plan = plan();
    for handle in [
        handle::ROLE,
        handle::GAME_MODE_CLASS,
        handle::CONNECTED_PLAYERS,
        handle::WORLD_TALENT_MANAGER_CLASS,
        handle::FLOD,
        handle::LAST + 1,
        1,
    ] {
        let mut state = state();
        state.values.insert(handle, RepValue::Bool(true));
        let mut cache = NetGuidCache::new();
        let mut exports = ConnectionExports::new();
        assert!(
            compose_game_state_open(
                &mut cache,
                &mut exports,
                &plan,
                &state,
                4,
                ByteOrder::Little
            )
            .is_err(),
            "handle {handle} accepted"
        );
        // Validation precedes every assignment.
        assert_eq!(cache.assign_dynamic().unwrap(), 2);
    }
    // A value of the wrong type fails in the layout encoder.
    let mut state = state();
    state
        .values
        .insert(handle::TIME_OF_DAY, RepValue::Bool(true));
    let mut cache = NetGuidCache::new();
    let mut exports = ConnectionExports::new();
    assert!(
        compose_game_state_open(
            &mut cache,
            &mut exports,
            &plan,
            &state,
            4,
            ByteOrder::Little
        )
        .is_err()
    );
}

#[test]
fn updates_carry_one_actor_block_in_handle_order() {
    let plan = plan();
    let values = BTreeMap::from([
        (handle::TIME_OF_DAY, RepValue::Float(635.25f32.to_bits())),
        (
            handle::WORLD_TIME_SECONDS,
            RepValue::Float(14.5f32.to_bits()),
        ),
        (handle::LEVEL_TIME_ELAPSED_SEC, RepValue::Int(440_000)),
    ]);
    let payload = compose_game_state_update(&plan.layout, 17, &values, ByteOrder::Little).unwrap();
    let body = read_actor_bunch_body(&mut payload.reader(), false, &codec()).unwrap();
    assert!(body.spawn.is_none());
    let [block] = body.blocks.as_slice() else {
        panic!("one block");
    };
    assert!(block.has_rep_layout && matches!(block.target, BlockTarget::Actor));
    let stream =
        read_rep_properties(&mut block.payload.reader(), &plan.layout, &rep_codec()).unwrap();
    assert!(
        stream
            .entries
            .windows(2)
            .all(|pair| pair[0].handle < pair[1].handle),
        "entries are written in handle order"
    );
    let decoded: BTreeMap<u32, RepValue> = stream
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect();
    assert_eq!(decoded, values);
}

#[test]
fn empty_skipped_out_of_range_and_mistyped_updates_are_rejected() {
    let plan = plan();
    let update = |values: BTreeMap<u32, RepValue>| {
        compose_game_state_update(&plan.layout, 17, &values, ByteOrder::Little)
    };
    assert!(update(BTreeMap::new()).is_err());
    for handle in [handle::WORLD_TALENT_MANAGER_CLASS, handle::LAST + 1, 1] {
        assert!(
            update(BTreeMap::from([(handle, RepValue::Bool(true))])).is_err(),
            "handle {handle} accepted"
        );
    }
    assert!(
        update(BTreeMap::from([(
            handle::TIME_OF_DAY,
            RepValue::Bool(true)
        )]))
        .is_err()
    );
}

#[test]
fn a_locally_saving_server_gathers_the_captured_networking_status() {
    use icarus_bevy::experimental::game_state_open::{NetworkingStatus, class_default};
    let first = handle::NETWORKING_STATUS_FIRST;
    let values = NetworkingStatus::local(600.0).values().unwrap();
    assert_eq!(values.len(), 12);
    let float = |value: f32| RepValue::Float(value.to_bits());
    let host: Vec<u16> = "Dedicated Server".encode_utf16().collect();
    assert!(matches!(&values[&first], RepValue::Str(text) if text.units() == host));
    for (offset, expected) in [
        (4, RepValue::Bool(true)),
        (5, float(600.0)),
        (6, float(-1.0)),
        (7, float(10.0)),
        (8, float(-1.0)),
        (9, float(720.0)),
    ] {
        assert_eq!(
            values[&(first + offset)],
            expected,
            "handle {}",
            first + offset
        );
    }
    // The backup host, both player arrays and the counters stay default.
    for offset in [1, 2, 3, 10, 11] {
        assert_eq!(
            Some(&values[&(first + offset)]),
            class_default(first + offset).as_ref(),
            "handle {}",
            first + offset
        );
    }
    // A short interval keeps the 180 s timeout floor.
    assert_eq!(
        NetworkingStatus::local(30.0).values().unwrap()[&(first + 9)],
        float(180.0)
    );
}

#[test]
fn listed_players_replicate_as_two_arrays() {
    use icarus_bevy::experimental::game_state_open::NetworkingStatus;
    let plan = plan();
    let first = handle::NETWORKING_STATUS_FIRST;
    let mut status = NetworkingStatus::local(600.0);
    status.player_names = vec![
        "Ada".encode_utf16().collect(),
        "Bo".encode_utf16().collect(),
    ];
    status.player_ping = vec![12, 0];
    let values: BTreeMap<u32, RepValue> = status
        .values()
        .unwrap()
        .into_iter()
        .filter(|(handle, _)| *handle == first + 2 || *handle == first + 3)
        .collect();
    let payload = compose_game_state_update(&plan.layout, 17, &values, ByteOrder::Little).unwrap();
    let body = read_actor_bunch_body(&mut payload.reader(), false, &codec()).unwrap();
    let stream = read_rep_properties(
        &mut body.blocks[0].payload.reader(),
        &plan.layout,
        &rep_codec(),
    )
    .unwrap();
    let decoded: BTreeMap<u32, RepValue> = stream
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect();
    assert_eq!(decoded, values);
    // Unequal arrays are rejected.
    status.player_ping.pop();
    assert!(status.values().is_err());
}

#[test]
fn networking_status_debug_omits_player_names() {
    use icarus_bevy::experimental::game_state_open::NetworkingStatus;
    let mut status = NetworkingStatus::local(600.0);
    status.player_names = vec!["SecretName".encode_utf16().collect()];
    status.player_ping = vec![1];
    let shown = format!("{status:?}");
    assert!(
        !shown.contains("SecretName") && shown.contains("players: 1"),
        "{shown}"
    );
}

/// `LockedMissions` as `UpdateLockedMissions` builds it (F-834): each lock
/// ends the retry timeout after the mission's end, or after `now` while it
/// is unfinished; ended locks drop out and an empty list is the default.
#[test]
fn locked_missions_list_each_lock_end_as_row_and_int_commands() {
    use icarus_bevy::experimental::game_state_open::{
        LockedMission, MISSION_RETRY_TIMEOUT_SECONDS, class_default, locked_missions_value,
    };
    let timeout = MISSION_RETRY_TIMEOUT_SECONDS;
    assert_eq!(timeout, 5 * 365 * 86_400);
    let missions = [
        LockedMission {
            row: 126,
            end_time: 23_727,
        },
        LockedMission {
            row: 36,
            end_time: -1,
        },
    ];
    let Some(RepValue::DynamicArray(array)) = locked_missions_value(&missions, 439_141) else {
        panic!("a locked missions array");
    };
    assert_eq!(array.count, 2);
    let entries: Vec<(u32, RepValue)> = array
        .stream
        .entries
        .iter()
        .map(|entry| (entry.handle, entry.value.clone()))
        .collect();
    assert_eq!(
        entries,
        [
            (1, RepValue::Byte(126)),
            (2, RepValue::Int(157_703_727)),
            (3, RepValue::Byte(36)),
            (4, RepValue::Int(158_119_141)),
        ]
    );
    // A lock lasts until its end time plus the timeout, inclusive.
    let ended = 23_727 + timeout;
    let Some(RepValue::DynamicArray(array)) = locked_missions_value(&missions, ended) else {
        panic!("a locked missions array");
    };
    assert_eq!(array.count, 2);
    let Some(RepValue::DynamicArray(array)) = locked_missions_value(&missions, ended + 1) else {
        panic!("a locked missions array");
    };
    assert_eq!(array.count, 1);
    assert_eq!(array.stream.entries[0].handle, 1);
    assert_eq!(array.stream.entries[0].value, RepValue::Byte(36));
    assert!(locked_missions_value(&missions[..1], ended + 1).is_none());
    assert!(locked_missions_value(&[], 0).is_none());
    assert!(matches!(
        class_default(handle::LOCKED_MISSIONS),
        Some(RepValue::DynamicArray(array)) if array.count == 0
    ));
}

#[test]
fn locked_missions_refresh_at_the_restore_and_every_minute() {
    use icarus_bevy::experimental::game_state_open::locked_missions_refreshed_at;
    assert_eq!(locked_missions_refreshed_at(150, 0.0), 150);
    assert_eq!(locked_missions_refreshed_at(150, 59.9), 150);
    assert_eq!(locked_missions_refreshed_at(150, 60.0), 210);
    assert_eq!(locked_missions_refreshed_at(150, 179.0), 270);
    assert_eq!(locked_missions_refreshed_at(150, -5.0), 150);
    assert_eq!(locked_missions_refreshed_at(i32::MAX, 120.0), i32::MAX);
}

#[test]
fn locked_missions_updates_round_trip_through_the_layout() {
    use icarus_bevy::experimental::game_state_open::{LockedMission, locked_missions_value};
    let plan = plan();
    let missions = [
        LockedMission {
            row: 90,
            end_time: 234_203,
        },
        LockedMission {
            row: 36,
            end_time: -1,
        },
    ];
    let values = BTreeMap::from([(
        handle::LOCKED_MISSIONS,
        locked_missions_value(&missions, 439_201).unwrap(),
    )]);
    let payload = compose_game_state_update(&plan.layout, 17, &values, ByteOrder::Little).unwrap();
    let body = read_actor_bunch_body(&mut payload.reader(), false, &codec()).unwrap();
    let [block] = body.blocks.as_slice() else {
        panic!("one block");
    };
    let stream =
        read_rep_properties(&mut block.payload.reader(), &plan.layout, &rep_codec()).unwrap();
    let decoded: BTreeMap<u32, RepValue> = stream
        .entries
        .into_iter()
        .map(|entry| (entry.handle, entry.value))
        .collect();
    assert_eq!(decoded, values);
}

#[test]
fn complete_active_prospect_values_round_trip_including_clears_in_both_orders() {
    use icarus_bevy::experimental::{
        game_state_open::{active_prospect_update_values, active_prospect_values, class_default},
        prospect_info::{AssociatedMember, CustomGameSetting, ProspectInfo},
    };
    let info = ProspectInfo {
        prospect_id: "authored".encode_utf16().collect(),
        claimed_account_character: i32::MIN,
        expire_time: i64::MAX,
        prospect_state: 4,
        associated_members: vec![AssociatedMember {
            account_name: "Æ".encode_utf16().collect(),
            chr_slot: i32::MAX,
            experience: i32::MIN,
            status: 12,
            is_currently_playing: true,
            ..Default::default()
        }],
        cost: i32::MAX,
        reward: i32::MIN,
        difficulty: 4,
        insurance: true,
        no_respawns: true,
        elapsed_time: i32::MAX,
        selected_drop_point: i32::MIN,
        custom_settings: vec![CustomGameSetting {
            setting_row_name: "setting".encode_utf16().collect(),
            setting_value: i32::MIN,
        }],
        ..Default::default()
    };
    let cleared = ProspectInfo::default();
    let defaults = active_prospect_update_values(&cleared).unwrap();
    assert_eq!(defaults.len(), 17);
    assert!(active_prospect_values(&cleared).unwrap().is_empty());
    for handle in handle::ACTIVE_PROSPECT {
        assert_eq!(defaults.get(&handle), class_default(handle).as_ref());
    }
    for current in [&info, &cleared] {
        let values = active_prospect_update_values(current).unwrap();
        assert_eq!(
            values.keys().copied().collect::<Vec<_>>(),
            handle::ACTIVE_PROSPECT.collect::<Vec<_>>()
        );
        let open = active_prospect_values(current).unwrap();
        assert!(
            open.iter()
                .all(|(handle, value)| values.get(handle) == Some(value))
        );
        for order in [ByteOrder::Little, ByteOrder::Big] {
            let mut actor = codec();
            actor.order = order;
            let mut rep = rep_codec();
            rep.actor = actor;
            let payload = compose_game_state_update(&plan().layout, 17, &values, order).unwrap();
            let body = read_actor_bunch_body(&mut payload.reader(), false, &actor).unwrap();
            let decoded: BTreeMap<u32, RepValue> =
                read_rep_properties(&mut body.blocks[0].payload.reader(), &plan().layout, &rep)
                    .unwrap()
                    .entries
                    .into_iter()
                    .map(|entry| (entry.handle, entry.value))
                    .collect();
            assert_eq!(decoded, values);
        }
    }
}

#[test]
fn active_prospect_values_reject_invalid_enums_strings_and_resource_counts() {
    use icarus_bevy::experimental::{
        game_state_open::active_prospect_update_values,
        prospect_info::{AssociatedMember, CustomGameSetting, ProspectInfo},
    };
    let invalid = [
        ProspectInfo {
            prospect_state: 5,
            ..Default::default()
        },
        ProspectInfo {
            difficulty: 5,
            ..Default::default()
        },
        ProspectInfo {
            associated_members: vec![AssociatedMember {
                status: 13,
                ..Default::default()
            }],
            ..Default::default()
        },
        ProspectInfo {
            prospect_id: vec![0],
            ..Default::default()
        },
        ProspectInfo {
            custom_settings: vec![CustomGameSetting {
                setting_row_name: vec![0xffff],
                setting_value: 0,
            }],
            ..Default::default()
        },
        ProspectInfo {
            lobby_name: vec![65; 65_536],
            ..Default::default()
        },
        ProspectInfo {
            associated_members: vec![AssociatedMember::default(); 512],
            ..Default::default()
        },
        ProspectInfo {
            custom_settings: vec![CustomGameSetting::default(); 4097],
            ..Default::default()
        },
    ];
    for info in invalid {
        assert!(active_prospect_update_values(&info).is_err());
    }
}
