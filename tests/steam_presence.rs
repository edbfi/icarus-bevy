use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use icarus_bevy::{
    experimental::server_query::{
        Request, Response, SplitAssembler, datagram_header, encode_request, parse_response,
        parse_split,
    },
    steam_presence::{
        Advert, AdvertPlayer, ICARUS_APP_ID, QueryResponder, SERVER_FLAG_ACTIVE,
        SERVER_FLAG_DEDICATED, SERVER_FLAG_LINUX, SERVER_FLAG_SECURE, version_tag,
    },
};

fn authored_advert() -> Advert {
    Advert {
        server_steam_id: 0x0140_0000_0000_0001,
        name: "authored-native".to_owned(),
        map: String::new(),
        game_port: 17_777,
        query_port: 27_015,
        max_players: 2,
        players: Vec::new(),
        version_tag: version_tag("3.0.34.159385-rel-DangerousHorizons").unwrap(),
        prospect: Some("authored".to_owned()),
        prospect_info: None,
    }
}

#[test]
fn version_tags_keep_the_numeric_prefix_with_the_observed_suffix() {
    assert_eq!(
        version_tag("3.0.34.159385-rel-DangerousHorizons").unwrap(),
        "3.0.34.159385-x-x"
    );
    assert!(version_tag("-rel").is_err());
    assert!(version_tag("v3-rel").is_err());
}

#[test]
fn listing_and_queries_share_one_direct_session_description() {
    let advert = authored_advert();
    advert.validate().unwrap();
    let id = advert.server_steam_id.to_string();
    assert_eq!(
        advert.keywords(),
        format!(
            "BUILDID:0,OWNINGID:{id},OWNINGNAME:authored-native,SESSIONFLAGS:683,P_s:_authored,G_s:3.0.34.159385-x-x"
        )
    );
    assert_eq!(
        advert.game_data(),
        format!(
            "OWNINGID:{id},OWNINGNAME:authored-native,SESSIONFLAGS:683,P_s:_authored,G_s:3.0.34.159385-x-x,CONMETHOD:Direct,P2PADDR:0.0.0.0,P2PPORT:17777"
        )
    );
    let keys: Vec<String> = advert
        .rules()
        .iter()
        .map(|rule| String::from_utf8(rule.key.clone()).unwrap())
        .collect();
    assert_eq!(
        keys,
        [
            "CONMETHOD",
            "D_s",
            "G_s",
            "L_s",
            "OWNINGID",
            "OWNINGNAME",
            "P2PADDR",
            "P2PPORT",
            "P_s",
            "SESSIONFLAGS"
        ]
    );
    assert_eq!(advert.rules()[0].value, b"Direct");

    let info = advert.info();
    assert_eq!(
        (
            info.protocol,
            info.app_id,
            info.server_type,
            info.environment
        ),
        (17, 0, b'd', b'l')
    );
    assert_eq!(info.game_id, Some(u64::from(ICARUS_APP_ID)));
    assert_eq!(info.port, Some(17_777));
    assert_eq!(info.keywords.as_deref(), Some(advert.keywords().as_bytes()));

    let data = advert.game_server_data(7);
    assert_eq!(data.revision, Some(7));
    assert_eq!(data.app_id, Some(ICARUS_APP_ID));
    assert_eq!(data.game_type.as_deref(), Some(advert.keywords().as_str()));
    assert_eq!(data.game_data.as_deref(), Some(advert.game_data().as_str()));
    assert_eq!(data.password, Some(false));
    assert_eq!(
        advert.server_type().flags,
        Some(SERVER_FLAG_ACTIVE | SERVER_FLAG_SECURE | SERVER_FLAG_DEDICATED | SERVER_FLAG_LINUX)
    );
}

#[test]
fn adverts_reject_values_that_would_corrupt_key_lists() {
    for name in ["", "a,b", "a:b", "a\nb", &"x".repeat(64)] {
        let advert = Advert {
            name: name.to_owned(),
            ..authored_advert()
        };
        assert!(advert.validate().is_err(), "{name:?}");
    }
    let crowded = Advert {
        players: vec![
            AdvertPlayer {
                steam_id: 1,
                name: "a".to_owned(),
                connected_seconds: 1.0,
            };
            3
        ],
        ..authored_advert()
    };
    assert!(crowded.validate().is_err());
    let info = Advert {
        prospect_info: Some("not base64!".to_owned()),
        ..authored_advert()
    };
    assert!(info.validate().is_err());
}

fn ask(
    responder: &mut QueryResponder,
    advert: &Advert,
    from: SocketAddr,
    request: Request,
    now: Instant,
) -> Response {
    let replies = responder
        .answer(advert, from, &encode_request(&request), now)
        .unwrap();
    if replies.len() == 1 && datagram_header(&replies[0]).unwrap() == -1 {
        return parse_response(&replies[0]).unwrap();
    }
    let mut assembler = SplitAssembler::default();
    let mut message = None;
    for reply in replies {
        message = assembler.push(parse_split(&reply).unwrap()).unwrap();
    }
    parse_response(&message.unwrap()).unwrap()
}

#[test]
fn every_query_needs_a_challenge_bound_to_source_and_window() {
    let start = Instant::now();
    let mut responder = QueryResponder::with_secret([7; 32], start);
    let advert = authored_advert();
    let from: SocketAddr = "198.51.100.7:40000".parse().unwrap();
    let other: SocketAddr = "198.51.100.8:40000".parse().unwrap();

    let Response::Challenge(challenge) = ask(
        &mut responder,
        &advert,
        from,
        Request::Info { challenge: None },
        start,
    ) else {
        panic!("first info request must be challenged");
    };
    assert!(challenge >= 0);
    let info = ask(
        &mut responder,
        &advert,
        from,
        Request::Info {
            challenge: Some(challenge),
        },
        start,
    );
    assert_eq!(info, Response::Info(advert.info()));

    // Another source, or a stale window, gets a fresh challenge instead.
    assert!(matches!(
        ask(
            &mut responder,
            &advert,
            other,
            Request::Rules { challenge },
            start
        ),
        Response::Challenge(_)
    ));
    let next_window = start + Duration::from_secs(31);
    assert_eq!(
        ask(
            &mut responder,
            &advert,
            from,
            Request::Players { challenge },
            next_window
        ),
        Response::Players(Vec::new())
    );
    let later = start + Duration::from_secs(61);
    assert!(matches!(
        ask(
            &mut responder,
            &advert,
            from,
            Request::Rules { challenge },
            later
        ),
        Response::Challenge(_)
    ));

    let Response::Challenge(current) = ask(
        &mut responder,
        &advert,
        from,
        Request::Rules { challenge: -1 },
        later,
    ) else {
        panic!("rules must be challenged");
    };
    assert_eq!(
        ask(
            &mut responder,
            &advert,
            from,
            Request::Rules { challenge: current },
            later
        ),
        Response::Rules(advert.rules())
    );

    // Malformed datagrams are ignored.
    assert!(
        responder
            .answer(&advert, from, b"\xff\xff\xff\xffZ", later)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn long_rule_sets_are_split_within_a_datagram() {
    let advert = Advert {
        prospect_info: Some("A".repeat(3_000)),
        ..authored_advert()
    };
    advert.validate().unwrap();
    let start = Instant::now();
    let mut responder = QueryResponder::with_secret([1; 32], start);
    let from: SocketAddr = "198.51.100.9:1".parse().unwrap();
    let Response::Challenge(challenge) = ask(
        &mut responder,
        &advert,
        from,
        Request::Rules { challenge: -1 },
        start,
    ) else {
        panic!("rules must be challenged");
    };
    let replies = responder
        .answer(
            &advert,
            from,
            &encode_request(&Request::Rules { challenge }),
            start,
        )
        .unwrap();
    assert!(replies.len() > 1);
    assert!(replies.iter().all(|reply| reply.len() <= 1_400));
    assert_eq!(
        ask(
            &mut responder,
            &advert,
            from,
            Request::Rules { challenge },
            start
        ),
        Response::Rules(advert.rules())
    );
}
