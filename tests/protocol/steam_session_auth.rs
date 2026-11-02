use icarus_bevy::{
    steam_auth::{AuthSessions, MAX_TICKET_BYTES, VerifiedTicket, decode_ticket_hex},
    steam_fetch::{
        frame::{self, emsg},
        proto::{AuthTicket, ClientAuthList, ClientTicketAuthComplete, ProtoHeader},
    },
};
use prost::Message as _;
use std::time::Duration;

#[test]
fn hexadecimal_ticket_conversion_is_bounded_and_strict() {
    let units = |text: &str| text.encode_utf16().collect::<Vec<_>>();
    assert_eq!(
        decode_ticket_hex(&units("00aAFF19")).unwrap(),
        [0, 0xaa, 0xff, 0x19]
    );
    for value in ["", "0", "0x", "GG", " 0", "00\n", "\0\0", "é0"] {
        let error = decode_ticket_hex(&units(value)).unwrap_err();
        assert!(format!("{error:#}").starts_with("invalid Steam ticket hex"));
    }
    assert_eq!(
        decode_ticket_hex(&units(&"aa".repeat(MAX_TICKET_BYTES)))
            .unwrap()
            .len(),
        MAX_TICKET_BYTES
    );
    assert!(decode_ticket_hex(&units(&"aa".repeat(MAX_TICKET_BYTES + 1))).is_err());
    for size in [0, 1, 55, MAX_TICKET_BYTES, MAX_TICKET_BYTES + 1] {
        assert!(
            VerifiedTicket::verify(&vec![0; size], 0x0110_0001_0000_002a, 1_149_460, 1500).is_err()
        );
    }
}

#[test]
fn callback_absence_is_preserved_and_auth_payload_debug_is_redacted() {
    let omitted = ClientTicketAuthComplete::decode(&[][..]).unwrap();
    assert_eq!(omitted.eauth_session_response, None);
    let success = ClientTicketAuthComplete {
        eauth_session_response: Some(0),
        ..ClientTicketAuthComplete::default()
    };
    assert_eq!(success.encode_to_vec(), [0x20, 0]);
    let list = ClientAuthList {
        tokens_left: Some(0),
        last_request_seq: Some(0),
        last_request_seq_from_server: Some(0),
        tickets: vec![AuthTicket {
            estate: Some(1),
            steamid: Some(0x0110_0001_0000_002a),
            gameid: Some(1_149_460),
            ticket_crc: Some(123),
            ticket: Some(b"do-not-log-ticket".to_vec()),
            ..AuthTicket::default()
        }],
        app_ids: vec![1_149_460],
        message_sequence: Some(1),
    };
    let debug = format!("{list:?}");
    assert!(!debug.contains("do-not-log-ticket"));
    assert!(!debug.contains("76561197960265770"));
    assert!(debug.contains("REDACTED"));
    let wire = frame::encode(
        emsg::CLIENT_AUTH_LIST,
        &ProtoHeader::default(),
        &list.encode_to_vec(),
    );
    let packet = frame::decode(&wire).unwrap();
    assert_eq!(packet.emsg, 5432);
    assert_eq!(
        ClientAuthList::decode(packet.body.as_slice()).unwrap(),
        list
    );
}

#[test]
fn authentication_resource_limits_are_explicit() {
    for (active, requests, seconds) in [
        (0, 1, 1),
        (65, 65, 1),
        (2, 1, 1),
        (1, 4097, 1),
        (1, 1, 0),
        (1, 1, 121),
    ] {
        assert!(
            AuthSessions::new(1_149_460, active, requests, Duration::from_secs(seconds)).is_err()
        );
    }
    assert!(AuthSessions::new(1_149_460, 64, 4096, Duration::from_secs(120)).is_ok());
}
