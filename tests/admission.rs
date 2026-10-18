use icarus_bevy::experimental::admission::{
    AdmissionPolicy, ClientLoginState, ConnectionLoginAdmission, DEFAULT_MAX_CLIENT_RATE,
    FIND_PLAYER_START_ERROR, GAMESESSION_NULL_ERROR, INCOMPATIBLE_UNIQUE_NET_ID,
    InitNewPlayerOutcome, JoinSpawnContext, MAX_PLAYER_NAME_UNITS, MIN_CLIENT_NETSPEED, Occupancy,
    PLAYER_STATE_NULL_ERROR, SERVER_FULL, SPAWN_PLAYER_CONTROLLER_ERROR, SPLITSCREEN_EXCEEDED,
    check_unique_net_id_compatible, clamp_client_netspeed, has_login_option, int_option,
    login_option, resolve_player_name, unique_net_id_is_valid,
};
use icarus_bevy::experimental::control_messages::{
    ArchiveText, NetIdType, NetIdValue, UniqueNetId,
};

// Independently authored option strings for the engine's login and join checks
// (F-712, F-713, F-740); no captured traffic.

fn units(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn value(options: &str, key: &str) -> Option<String> {
    let options = units(options);
    login_option(&options, key).map(String::from_utf16_lossy)
}

const POLICY: AdmissionPolicy = AdmissionPolicy {
    max_players: 8,
    max_spectators: 2,
    max_splitscreens_per_connection: 1,
};

#[test]
fn options_are_question_mark_delimited_from_their_start() {
    assert_eq!(value("?Name=Ada?Listen", "Name").as_deref(), Some("Ada"));
    assert_eq!(value("?Name=Ada?Listen", "Listen").as_deref(), Some(""));
    assert_eq!(value("?Name=Ada", "Missing"), None);
    // Without a leading delimiter the engine reads no option at all.
    assert_eq!(value("Name=Ada?Team=1", "Team"), None);
    assert_eq!(value("", "Name"), None);
}

#[test]
fn values_split_at_the_first_equals_and_the_first_match_wins() {
    assert_eq!(value("?Key=a=b", "Key").as_deref(), Some("a=b"));
    assert_eq!(value("?Key=1?Key=2", "Key").as_deref(), Some("1"));
    assert_eq!(value("??Key=3", "Key").as_deref(), Some("3"));
    assert_eq!(value("?=x?Key", "").as_deref(), Some("x"));
}

#[test]
fn keys_compare_ascii_case_insensitively_and_other_units_exactly() {
    assert_eq!(
        value("?spectatoronly=1", "SpectatorOnly").as_deref(),
        Some("1")
    );
    assert_eq!(
        value("?SPLITSCREENCOUNT=2", "SplitscreenCount").as_deref(),
        Some("2")
    );
    assert_eq!(value("?\u{00dc}=1", "\u{00fc}"), None);
    assert_eq!(value("?\u{00dc}=1", "\u{00dc}").as_deref(), Some("1"));
    assert_eq!(value("?Name1=x", "Name"), None);
}

#[test]
fn integer_options_read_like_the_c_runtime() {
    let read = |options: &str, key: &str| int_option(&units(options), key);
    assert_eq!(read("?N=42", "N"), 42);
    assert_eq!(read("?N= \t-7x", "N"), -7);
    assert_eq!(read("?N=+3", "N"), 3);
    assert_eq!(read("?N=x1", "N"), 0);
    assert_eq!(read("?N=", "N"), 0);
    assert_eq!(read("?M=5", "N"), 0);
    assert_eq!(read("?N=99999999999", "N"), i32::MAX);
    assert_eq!(read("?N=-2147483648", "N"), i32::MIN);
    assert_eq!(read("?N=-99999999999", "N"), i32::MIN);
    // Full-width digits are not decimal digits here.
    assert_eq!(read("?N=\u{ff11}", "N"), 0);
}

#[test]
fn players_are_capped_only_by_a_positive_maximum() {
    let full = Occupancy {
        players: 8,
        spectators: 0,
    };
    assert_eq!(
        POLICY.approve_login(&units("?Name=A"), full),
        Err(SERVER_FULL)
    );
    let room = Occupancy {
        players: 7,
        spectators: 0,
    };
    assert_eq!(POLICY.approve_login(&units("?Name=A"), room), Ok(()));
    let uncapped = AdmissionPolicy {
        max_players: 0,
        ..POLICY
    };
    let crowd = Occupancy {
        players: 1_000,
        spectators: 0,
    };
    assert_eq!(uncapped.approve_login(&units(""), crowd), Ok(()));
}

#[test]
fn spectator_only_logins_count_spectators_instead() {
    let players_full = Occupancy {
        players: 8,
        spectators: 1,
    };
    assert_eq!(
        POLICY.approve_login(&units("?SpectatorOnly=1"), players_full),
        Ok(())
    );
    let spectators_full = Occupancy {
        players: 0,
        spectators: 2,
    };
    assert_eq!(
        POLICY.approve_login(&units("?SpectatorOnly=1"), spectators_full),
        Err(SERVER_FULL)
    );
    // Only the value 1 asks for a spectator slot.
    assert_eq!(
        POLICY.approve_login(&units("?SpectatorOnly=2"), spectators_full),
        Ok(())
    );
}

#[test]
fn splitscreen_above_the_connection_limit_is_rejected_after_capacity() {
    let room = Occupancy::default();
    assert_eq!(
        POLICY.approve_login(&units("?SplitscreenCount=1"), room),
        Ok(())
    );
    assert_eq!(
        POLICY.approve_login(&units("?SplitscreenCount=2"), room),
        Err(SPLITSCREEN_EXCEEDED)
    );
    let full = Occupancy {
        players: 8,
        spectators: 0,
    };
    assert_eq!(
        POLICY.approve_login(&units("?SplitscreenCount=2"), full),
        Err(SERVER_FULL)
    );
}

#[test]
fn pre_login_checks_unique_net_id_compatibility_before_approve_login() {
    let empty_id = UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("UNKNOWN").unwrap()),
        value: NetIdValue::EncodedEmpty,
    };
    assert!(!unique_net_id_is_valid(&empty_id));
    // An invalid/empty unique net id skips IsCompatibleUniqueNetId and runs ApproveLogin.
    assert_eq!(
        POLICY.pre_login(
            &units("?SplitscreenCount=1"),
            &empty_id,
            Occupancy::default(),
            Some("STEAM"),
            &["EOS"],
            &[],
        ),
        Ok(())
    );
    let steam_id = UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("steam").unwrap()),
        value: NetIdValue::from_hex("0123456789ABCDEF").unwrap(),
    };
    assert!(unique_net_id_is_valid(&steam_id));
    assert_eq!(
        check_unique_net_id_compatible(&steam_id, Some("STEAM"), &["EOS"], &[]),
        Ok(())
    );
    let incompatible_id = UniqueNetId {
        kind: NetIdType::Named(ArchiveText::text("PSN").unwrap()),
        value: NetIdValue::from_hex("0123456789ABCDEF").unwrap(),
    };
    // Incompatible net id fails with INCOMPATIBLE_UNIQUE_NET_ID even when server is full.
    let full = Occupancy {
        players: 8,
        spectators: 0,
    };
    assert_eq!(
        POLICY.pre_login(
            &units("?SplitscreenCount=2"),
            &incompatible_id,
            full,
            Some("STEAM"),
            &["EOS"],
            &[],
        ),
        Err(INCOMPATIBLE_UNIQUE_NET_ID)
    );
}

#[test]
fn connection_login_admission_gates_control_messages_by_state() {
    let mut conn = ConnectionLoginAdmission::new();
    assert_eq!(conn.state, ClientLoginState::LoggingIn);
    assert_eq!(conn.expected_login_msg_type, 0);
    // Initially only NMT_Hello (0) is accepted.
    assert!(conn.is_client_msg_type_valid(0));
    assert!(!conn.is_client_msg_type_valid(4));
    assert!(!conn.is_client_msg_type_valid(5));
    assert!(!conn.is_client_msg_type_valid(9));

    // After SendChallengeControlMessage (0x143998730), only NMT_Login (5) is accepted.
    conn.on_challenge_sent();
    assert_eq!(conn.expected_login_msg_type, 5);
    assert!(!conn.is_client_msg_type_valid(0));
    assert!(!conn.is_client_msg_type_valid(4));
    assert!(conn.is_client_msg_type_valid(5));
    assert!(!conn.is_client_msg_type_valid(9));

    // After WelcomePlayer (0x143cc9170), Hello (0) and Login (5) are rejected,
    // while Netspeed (4), Join (9), JoinSplit (10), PCSwap (15), DebugText (17) pass.
    conn.on_welcome_sent();
    assert_eq!(conn.state, ClientLoginState::Welcomed);
    assert!(!conn.is_client_msg_type_valid(0));
    assert!(!conn.is_client_msg_type_valid(5));
    for id in [4, 9, 10, 15, 17] {
        assert!(conn.is_client_msg_type_valid(id));
    }

    // First NMT_Join spawns the PlayerController and advances to Joined (3);
    // duplicate NMT_Join is ignored when PlayerController is already set.
    assert!(conn.on_join_spawned());
    assert_eq!(conn.state, ClientLoginState::Joined);
    assert!(!conn.on_join_spawned());
    assert!(!conn.is_client_msg_type_valid(0));
    assert!(!conn.is_client_msg_type_valid(5));
    assert!(conn.is_client_msg_type_valid(4));
    assert!(conn.is_client_msg_type_valid(15));
}

#[test]
fn clamp_client_netspeed_matches_notify_control_message_case_4() {
    assert_eq!(MIN_CLIENT_NETSPEED, 1_800);
    assert_eq!(DEFAULT_MAX_CLIENT_RATE, 15_000);

    // Below 1800 (including 0, negative values, and u32 > i32::MAX cast to i32) returns 1800.
    assert_eq!(clamp_client_netspeed(0, DEFAULT_MAX_CLIENT_RATE), 1_800);
    assert_eq!(clamp_client_netspeed(1_799, DEFAULT_MAX_CLIENT_RATE), 1_800);
    assert_eq!(clamp_client_netspeed(-1, DEFAULT_MAX_CLIENT_RATE), 1_800);
    assert_eq!(
        clamp_client_netspeed(i32::MIN, DEFAULT_MAX_CLIENT_RATE),
        1_800
    );
    assert_eq!(
        clamp_client_netspeed(0x8000_0000_u32 as i32, DEFAULT_MAX_CLIENT_RATE),
        1_800
    );
    assert_eq!(
        clamp_client_netspeed(u32::MAX as i32, DEFAULT_MAX_CLIENT_RATE),
        1_800
    );

    // Within [1800, max_client_rate] returns requested_rate.
    assert_eq!(clamp_client_netspeed(1_800, DEFAULT_MAX_CLIENT_RATE), 1_800);
    assert_eq!(
        clamp_client_netspeed(10_000, DEFAULT_MAX_CLIENT_RATE),
        10_000
    );
    assert_eq!(
        clamp_client_netspeed(15_000, DEFAULT_MAX_CLIENT_RATE),
        15_000
    );

    // Above max_client_rate returns max_client_rate.
    assert_eq!(
        clamp_client_netspeed(15_001, DEFAULT_MAX_CLIENT_RATE),
        15_000
    );
    assert_eq!(
        clamp_client_netspeed(100_000, DEFAULT_MAX_CLIENT_RATE),
        15_000
    );
    assert_eq!(
        clamp_client_netspeed(i32::MAX, DEFAULT_MAX_CLIENT_RATE),
        15_000
    );

    // Exact assembly branch order (`cmp ecx, 1800; jl -> 1800; else min(ecx, MaxClientRate)`):
    // when max_client_rate < 1800, requested_rate < 1800 still returns 1800, while
    // requested_rate >= 1800 returns max_client_rate.
    assert_eq!(clamp_client_netspeed(1_799, 1_000), 1_800);
    assert_eq!(clamp_client_netspeed(1_800, 1_000), 1_000);
    assert_eq!(clamp_client_netspeed(5_000, 1_000), 1_000);
}

#[test]
fn login_and_init_new_player_orders_errors_and_truncates_player_names() {
    let base_ctx = JoinSpawnContext {
        has_game_session: true,
        can_spawn_controller: true,
        has_player_state: true,
        has_player_start: true,
        must_spectate: false,
        default_player_name: "Player",
        player_id: 256,
        net_player_index: 0,
    };

    assert_eq!(
        POLICY.login_and_init_new_player(
            &units("?Name=Ada"),
            Occupancy::default(),
            JoinSpawnContext {
                has_game_session: false,
                ..base_ctx
            },
        ),
        Err(GAMESESSION_NULL_ERROR)
    );
    assert_eq!(
        POLICY.login_and_init_new_player(
            &units("?SplitscreenCount=2"),
            Occupancy::default(),
            base_ctx,
        ),
        Err(SPLITSCREEN_EXCEEDED)
    );
    assert_eq!(
        POLICY.login_and_init_new_player(
            &units("?Name=Ada"),
            Occupancy::default(),
            JoinSpawnContext {
                can_spawn_controller: false,
                ..base_ctx
            },
        ),
        Err(SPAWN_PLAYER_CONTROLLER_ERROR)
    );
    assert_eq!(
        POLICY.login_and_init_new_player(
            &units("?Name=Ada"),
            Occupancy::default(),
            JoinSpawnContext {
                has_player_state: false,
                ..base_ctx
            },
        ),
        Err(PLAYER_STATE_NULL_ERROR)
    );
    assert_eq!(
        POLICY.login_and_init_new_player(
            &units("?Name=Ada"),
            Occupancy::default(),
            JoinSpawnContext {
                has_player_start: false,
                ..base_ctx
            },
        ),
        Err(FIND_PLAYER_START_ERROR)
    );

    // Long ?Name= is truncated to 20 UTF-16 units (0x14), and ?bIsFromInvite is detected.
    let opts = units("?Name=0123456789abcdefghijEXTRA?bIsFromInvite?SpectatorOnly=1");
    assert!(has_login_option(&opts, "bisfrominvite"));
    let outcome = POLICY
        .login_and_init_new_player(&opts, Occupancy::default(), base_ctx)
        .unwrap();
    assert_eq!(
        outcome,
        InitNewPlayerOutcome {
            is_from_invite: true,
            spectator_only: true,
            player_name_units: units("0123456789abcdefghij"),
            net_player_index: 0,
        }
    );
    assert_eq!(outcome.player_name_units.len(), MAX_PLAYER_NAME_UNITS);

    // Empty or absent ?Name= falls back to "{DefaultPlayerName}{PlayerId}".
    assert_eq!(
        resolve_player_name(&units("?Name="), "Player", 256),
        units("Player256")
    );
}
