//! The private pawn-channel tap leaves the session byte-identical: two peers
//! from one fixed cookie receive the same authored client datagrams, one
//! with the tap and one without, and every datagram each sends is compared
//! byte for byte. Authored bits only.
use super::*;
use crate::experimental::{
    packet::{BitWriter, BunchHeader, Name, Packet, PacketConfig, Receiver, write_packet},
    selected_rpc_capture::pawn_bunches::{PawnBunchTap, PrivatePawnBunchCapture},
    sequencing::wire_reliable_sequence,
};

const PAWN: u32 = 2;
const OTHER: u32 = 3;
const COOKIE: [u8; COOKIE_BYTES] = [
    0x34, 0x12, 0x05, 0xc4, 9, 8, 7, 6, 5, 4, 3, 2, 1, 0, 0xaa, 0xbb, 0xcc, 0xdd, 0xee, 0xff,
];

fn limits() -> BunchLimits {
    BunchLimits {
        max_channels: 16,
        max_packet_bytes: 1024,
        max_string_units: 64,
    }
}

fn bunch(channel: u32, reliable: Option<u32>, marker: u8, open: bool) -> Bunch {
    let mut writer = BitWriter::new();
    writer.write_bytes(&[marker, marker ^ 0x5a]);
    writer.write_bit(true);
    Bunch {
        header: BunchHeader {
            control: open,
            open,
            close: false,
            close_reason: None,
            paused: false,
            reliable: reliable.is_some() || open,
            channel_index: channel,
            has_package_map_exports: false,
            has_must_be_mapped_guids: false,
            partial: false,
            reliable_sequence_raw: reliable,
            partial_initial: false,
            partial_final: false,
            channel_name: (open || reliable.is_some()).then_some(Name::Hardcoded(102)),
        },
        payload: Payload::from_writer(writer),
    }
}

fn part(channel: u32, sequence: u32, initial: bool, last: bool) -> Bunch {
    let mut part = bunch(channel, Some(wire_reliable_sequence(sequence)), 0x40, false);
    part.header.partial = true;
    part.header.partial_initial = initial;
    part.header.partial_final = last;
    part
}

/// Everything one peer emits and reports over the authored client stream.
fn run(tapped: bool) -> (Vec<Vec<u8>>, Vec<String>, u64) {
    let config = PacketLabConfig::new(limits());
    let mut peer = LabPeer::from_cookie(&COOKIE, config).unwrap();
    let (incoming, outgoing) = client_cookie_seeds(&COOKIE);
    let mut client = PacketSequencer::new(incoming, outgoing, 64).unwrap();
    let mut sent = Vec::new();
    for channel in [PAWN, OTHER] {
        sent.push(
            peer.reliable_actor_open(&bunch(channel, None, 1, true), &[])
                .unwrap(),
        );
    }
    let first = peer.sequencer().initial_reliable().0 + 1;
    let seq = |offset: u32| Some(wire_reliable_sequence(first + offset));
    let mut unsupported = bunch(PAWN, seq(4), 7, false);
    unsupported.header.has_package_map_exports = true;
    let streams: Vec<Vec<Bunch>> = vec![
        // Out of order, a duplicate in the same datagram, another channel.
        vec![
            bunch(PAWN, seq(1), 2, false),
            bunch(PAWN, seq(1), 2, false),
            bunch(OTHER, seq(0), 9, false),
        ],
        vec![bunch(PAWN, seq(0), 3, false), bunch(PAWN, None, 4, false)],
        // A two-part bunch, then a form the session drops.
        vec![part(PAWN, first + 2, true, false)],
        vec![part(PAWN, first + 3, false, true), unsupported],
        vec![bunch(PAWN, None, 4, false), bunch(OTHER, None, 5, false)],
    ];
    let to_server = PacketConfig {
        magic_bits: 0,
        receiver: Receiver::Server,
        limits: limits(),
    };
    let mut reports = Vec::new();
    let mut sink = PrivatePawnBunchCapture::from_file(tempfile::tempfile().unwrap());
    for bunches in streams {
        let bytes = write_packet(
            &Packet {
                magic: 0,
                header: client.header().unwrap(),
                info: None,
                bunches,
            },
            &to_server,
        )
        .unwrap();
        client.commit().unwrap();
        let mut tap = PawnBunchTap {
            sink: &mut sink,
            channel: PAWN,
            connection_ordinal: 1,
            elapsed_ms: 0,
            unix_ms: 0,
        };
        let received = peer
            .receive_with_capture(&bytes, None, tapped.then_some(&mut tap))
            .unwrap();
        reports.push(format!(
            "{:?} {:?} {:?}",
            received.summary(),
            received.actor_bunches(),
            received.unreliable_actor_bunches()
        ));
        sent.push(peer.ack_packet().unwrap());
    }
    (sent, reports, sink.finish().observations)
}

#[test]
fn the_pawn_tap_leaves_every_sent_datagram_byte_identical() {
    let (plain, plain_reports, none) = run(false);
    let (tapped, tapped_reports, kept) = run(true);
    assert_eq!(none, 0);
    // Released pawn reliables (two whole, two parts, the unsupported one)
    // and the two pawn unreliables; never the duplicate or other channel.
    assert_eq!(kept, 7);
    assert_eq!(plain, tapped);
    assert_eq!(plain_reports, tapped_reports);
}
