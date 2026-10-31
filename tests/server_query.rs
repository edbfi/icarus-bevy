use icarus_bevy::experimental::server_query::{
    Info, MAX_DATAGRAM, Player, REQUEST_CHALLENGE, Request, Response, Rule, SIMPLE_HEADER,
    SPLIT_HEADER, Spectator, SplitAssembler, datagram_header, encode_request, encode_response,
    parse_request, parse_response, parse_split, split_response,
};

fn authored_info() -> Info {
    Info {
        protocol: 17,
        name: b"authored server".to_vec(),
        map: b"authored map".to_vec(),
        folder: b"authored".to_vec(),
        game: b"Authored Game".to_vec(),
        app_id: 0x1234,
        players: 1,
        max_players: 8,
        bots: 0,
        server_type: b'd',
        environment: b'w',
        visibility: 1,
        vac: 0,
        version: b"1.2.3".to_vec(),
        port: Some(17_777),
        steam_id: Some(0x0110_0001_0000_0001),
        spectator: None,
        keywords: Some(b"a,b".to_vec()),
        game_id: Some(0x0000_0000_0012_3456),
    }
}

#[test]
fn info_requests_carry_the_fixed_payload_and_an_optional_challenge() {
    let first = encode_request(&Request::Info { challenge: None });
    assert_eq!(&first[..5], &[0xff, 0xff, 0xff, 0xff, b'T']);
    assert_eq!(&first[5..], b"Source Engine Query\0");
    assert_eq!(
        parse_request(&first).unwrap(),
        Request::Info { challenge: None }
    );

    let retry = encode_request(&Request::Info {
        challenge: Some(0x22d5_a14b),
    });
    assert_eq!(&retry[first.len()..], &[0x4b, 0xa1, 0xd5, 0x22]);
    assert_eq!(
        parse_request(&retry).unwrap(),
        Request::Info {
            challenge: Some(0x22d5_a14b)
        }
    );

    for request in [
        Request::Players {
            challenge: REQUEST_CHALLENGE,
        },
        Request::Rules { challenge: 7 },
    ] {
        let bytes = encode_request(&request);
        assert_eq!(bytes.len(), 9);
        assert_eq!(parse_request(&bytes).unwrap(), request);
    }
}

#[test]
fn malformed_requests_fail_closed() {
    let mut wrong_payload = encode_request(&Request::Info { challenge: None });
    wrong_payload[6] ^= 1;
    let mut truncated_challenge = encode_request(&Request::Info { challenge: Some(1) });
    truncated_challenge.pop();
    let mut trailing = encode_request(&Request::Rules { challenge: 1 });
    trailing.push(0);
    let mut split = encode_request(&Request::Rules { challenge: 1 });
    split[0] = 0xfe;
    let unknown = [0xff, 0xff, 0xff, 0xff, b'W', 0, 0, 0, 0];
    for bytes in [
        wrong_payload,
        truncated_challenge,
        trailing,
        split,
        unknown.to_vec(),
        vec![0xff; 3],
        vec![0xff; MAX_DATAGRAM + 1],
    ] {
        assert!(parse_request(&bytes).is_err(), "{bytes:02x?}");
    }
}

#[test]
fn info_answers_round_trip_with_their_extra_data_flags() {
    let info = authored_info();
    assert_eq!(info.extra_data_flags(), 0x80 | 0x10 | 0x20 | 0x01);
    let bytes = encode_response(&Response::Info(info.clone())).unwrap();
    assert_eq!(&bytes[..6], &[0xff, 0xff, 0xff, 0xff, b'I', 17]);
    assert_eq!(
        parse_response(&bytes).unwrap(),
        Response::Info(info.clone())
    );

    // Spectator data sits between the Steam ID and the keywords.
    let spectator = Info {
        spectator: Some(Spectator {
            port: 27_020,
            name: b"tv".to_vec(),
        }),
        ..info.clone()
    };
    let bytes = encode_response(&Response::Info(spectator.clone())).unwrap();
    assert_eq!(parse_response(&bytes).unwrap(), Response::Info(spectator));

    // A server may end the answer after the version string.
    let bare = Info {
        port: None,
        steam_id: None,
        keywords: None,
        game_id: None,
        ..info
    };
    let mut bytes = encode_response(&Response::Info(bare.clone())).unwrap();
    assert_eq!(bytes.pop(), Some(0));
    assert_eq!(parse_response(&bytes).unwrap(), Response::Info(bare));
}

#[test]
fn info_answers_reject_unknown_flags_truncation_and_trailing_bytes() {
    let mut unknown_flag = encode_response(&Response::Info(Info {
        port: None,
        steam_id: None,
        keywords: None,
        game_id: None,
        ..authored_info()
    }))
    .unwrap();
    *unknown_flag.last_mut().unwrap() = 0x02;
    let full = encode_response(&Response::Info(authored_info())).unwrap();
    let mut trailing = full.clone();
    trailing.push(0);
    for bytes in [unknown_flag, full[..full.len() - 1].to_vec(), trailing] {
        assert!(parse_response(&bytes).is_err());
    }
    let interior = Info {
        name: b"a\0b".to_vec(),
        ..authored_info()
    };
    assert!(encode_response(&Response::Info(interior)).is_err());
}

#[test]
fn challenges_players_and_rules_round_trip() {
    let challenge = encode_response(&Response::Challenge(0x22d5_a14b)).unwrap();
    assert_eq!(
        challenge,
        [0xff, 0xff, 0xff, 0xff, b'A', 0x4b, 0xa1, 0xd5, 0x22]
    );
    assert_eq!(
        parse_response(&challenge).unwrap(),
        Response::Challenge(0x22d5_a14b)
    );

    let players = Response::Players(vec![Player {
        index: 0,
        name: b"authored".to_vec(),
        score: -2,
        duration: 61.5,
    }]);
    let bytes = encode_response(&players).unwrap();
    assert_eq!(bytes[5], 1);
    assert_eq!(parse_response(&bytes).unwrap(), players);

    let rules = Response::Rules(vec![
        Rule {
            key: b"KEY_s".to_vec(),
            value: b"value".to_vec(),
        },
        Rule {
            key: b"EMPTY".to_vec(),
            value: Vec::new(),
        },
    ]);
    let bytes = encode_response(&rules).unwrap();
    assert_eq!(&bytes[5..7], &[2, 0]);
    assert_eq!(parse_response(&bytes).unwrap(), rules);

    // A count the message cannot hold fails before reading.
    let mut overcount = bytes;
    overcount[5] = 0xff;
    assert!(parse_response(&overcount).is_err());
}

#[test]
fn split_responses_reassemble_in_any_order_and_reject_mixing() {
    let rules = Response::Rules(
        (0..200)
            .map(|index| Rule {
                key: format!("authored_key_{index}").into_bytes(),
                value: vec![b'v'; 10],
            })
            .collect(),
    );
    let message = encode_response(&rules).unwrap();
    assert!(message.len() > MAX_DATAGRAM);
    let datagrams = split_response(&message, 9, 1_000).unwrap();
    assert!(datagrams.len() > 1);
    assert!(
        datagrams
            .iter()
            .all(|datagram| datagram.len() <= MAX_DATAGRAM)
    );
    assert!(
        datagrams
            .iter()
            .all(|datagram| datagram_header(datagram).unwrap() == SPLIT_HEADER)
    );

    let mut assembler = SplitAssembler::default();
    let mut reassembled = None;
    for datagram in datagrams.iter().rev() {
        assert!(reassembled.is_none());
        reassembled = assembler.push(parse_split(datagram).unwrap()).unwrap();
    }
    assert_eq!(reassembled.as_deref(), Some(message.as_slice()));
    assert_eq!(parse_response(&message).unwrap(), rules);

    let mut assembler = SplitAssembler::default();
    assert!(
        assembler
            .push(parse_split(&datagrams[0]).unwrap())
            .unwrap()
            .is_none()
    );
    assert!(assembler.push(parse_split(&datagrams[0]).unwrap()).is_err());
    let other = split_response(&message, 10, 1_000).unwrap();
    assert!(assembler.push(parse_split(&other[1]).unwrap()).is_err());

    // A message that fits stays unsplit.
    let small = encode_response(&Response::Challenge(1)).unwrap();
    let unsplit = split_response(&small, 1, 1_000).unwrap();
    assert_eq!(unsplit, vec![small.clone()]);
    assert_eq!(datagram_header(&unsplit[0]).unwrap(), SIMPLE_HEADER);
}

#[test]
fn split_slices_reject_compression_and_bad_numbering() {
    let mut compressed = SPLIT_HEADER.to_le_bytes().to_vec();
    compressed.extend_from_slice(&(-5i32).to_le_bytes());
    compressed.extend_from_slice(&[2, 0, 0xe0, 0x04, 1]);
    let mut numbering = SPLIT_HEADER.to_le_bytes().to_vec();
    numbering.extend_from_slice(&5i32.to_le_bytes());
    numbering.extend_from_slice(&[2, 2, 0xe0, 0x04, 1]);
    let mut empty = numbering.clone();
    empty[8] = 0;
    empty[9] = 0;
    for bytes in [compressed, numbering, empty] {
        assert!(parse_split(&bytes).is_err());
    }
    assert!(split_response(&[0; 2_000], -1, 1_000).is_err());
    assert!(split_response(&[0; 2_000], 1, 0).is_err());
}
