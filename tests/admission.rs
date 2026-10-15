use icarus_bevy::experimental::admission::{
    AdmissionPolicy, Occupancy, SERVER_FULL, SPLITSCREEN_EXCEEDED, int_option, login_option,
};

// Independently authored option strings for the engine's login checks
// (F-622); no captured traffic.

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
    assert_eq!(value("?Ü=1", "ü"), None);
    assert_eq!(value("?Ü=1", "Ü").as_deref(), Some("1"));
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
