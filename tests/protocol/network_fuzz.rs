//! Deterministic randomized robustness checks for the experimental packet and
//! control-message readers, the untrusted ingress surface. Inputs are random
//! bytes and mutations of datagrams and bodies built from authored values; no
//! captured traffic. These readers model the original's lenient acceptance
//! (capped history counts, non-canonical packed integers, control bunches
//! without open or close), so the checks are semantic: no input panics, and
//! every accepted input the writer can express re-encodes to a canonical form
//! that reads back to the same value and re-encodes to itself.
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::control_messages::{
    ArchiveText, ControlCodec, ControlMessage, Decoded, NetIdType, NetIdValue, UniqueNetId,
    encode_control_message, read_control_messages,
};
use icarus_bevy::experimental::packet::{
    BitReader, BitWriter, Bunch, BunchHeader, BunchLimits, Name, Packet, PacketConfig,
    PacketHeader, PacketInfo, Payload, Receiver, read_packet, write_packet,
};

struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_f491_4f6c_dd1d)
    }

    fn below(&mut self, bound: usize) -> usize {
        (self.next() % bound as u64) as usize
    }
}

fn mutate(rng: &mut Rng, corpus: &[Vec<u8>]) -> Vec<u8> {
    if rng.below(5) == 0 {
        let len = rng.below(96);
        return (0..len).map(|_| rng.next() as u8).collect();
    }
    let mut bytes = corpus[rng.below(corpus.len())].clone();
    match rng.below(4) {
        0 => {
            let at = rng.below(bytes.len() + 1);
            bytes.truncate(at);
        }
        1 => {
            for _ in 0..=rng.below(6) {
                bytes.push(rng.next() as u8);
            }
        }
        _ => {}
    }
    for _ in 0..=rng.below(3) {
        if !bytes.is_empty() {
            let at = rng.below(bytes.len());
            bytes[at] ^= 1 << rng.below(8);
        }
    }
    bytes
}

const LIMITS: BunchLimits = BunchLimits {
    max_channels: 16,
    max_packet_bytes: 64,
    max_string_units: 64,
};

fn payload(length: usize) -> Payload {
    let mut writer = BitWriter::new();
    for index in 0..length {
        writer.write_bit((index * 7 + index / 3) % 5 < 2);
    }
    Payload::from_writer(writer)
}

fn header() -> BunchHeader {
    BunchHeader {
        control: false,
        open: false,
        close: false,
        close_reason: None,
        paused: false,
        reliable: false,
        channel_index: 0,
        has_package_map_exports: false,
        has_must_be_mapped_guids: false,
        partial: false,
        reliable_sequence_raw: None,
        partial_initial: false,
        partial_final: false,
        channel_name: None,
    }
}

fn bunches() -> Vec<Bunch> {
    let mut open = header();
    (open.control, open.open, open.channel_index) = (true, true, 3);
    open.channel_name = Some(Name::Hardcoded(255));
    let mut close = header();
    (close.control, close.close, close.close_reason) = (true, true, Some(14));
    let mut reliable = header();
    (reliable.reliable, reliable.reliable_sequence_raw) = (true, Some(1023));
    (reliable.partial, reliable.partial_initial) = (true, true);
    reliable.channel_index = 15;
    reliable.channel_name = Some(Name::string(&[0x43, 0x6f], 0).unwrap());
    vec![
        Bunch {
            header: open,
            payload: payload(37),
        },
        Bunch {
            header: close,
            payload: payload(0),
        },
        Bunch {
            header: reliable,
            payload: payload(70),
        },
        Bunch {
            header: header(),
            payload: payload(8),
        },
    ]
}

fn config(receiver: Receiver) -> PacketConfig {
    PacketConfig {
        magic_bits: 3,
        receiver,
        limits: LIMITS,
    }
}

fn packets(receiver: Receiver) -> Vec<Vec<u8>> {
    let all = bunches();
    (0..=all.len())
        .map(|take| Packet {
            magic: 0b101,
            header: PacketHeader {
                sequence: 17 + take as u16,
                acked_sequence: 16,
                history: vec![1; 1 + take % 3],
            },
            info: (take % 2 == 0).then_some(PacketInfo {
                jitter_clock: 77,
                has_frame_time: receiver == Receiver::Client,
                server_frame_time: (receiver == Receiver::Client).then_some(9),
            }),
            bunches: all.iter().take(take).cloned().collect(),
        })
        .map(|packet| write_packet(&packet, &config(receiver)).unwrap())
        .collect()
}

#[test]
fn datagrams_never_panic_and_expressible_ones_re_encode() {
    let mut rng = Rng(0xbad0_0dd5_eed5_0017);
    let (mut accepted, mut identical) = (0usize, 0usize);
    for receiver in [Receiver::Client, Receiver::Server] {
        let config = config(receiver);
        let corpus = packets(receiver);
        for _ in 0..20_000 {
            let datagram = mutate(&mut rng, &corpus);
            let Ok(packet) = read_packet(&datagram, &config) else {
                continue;
            };
            accepted += 1;
            let Ok(encoded) = write_packet(&packet, &config) else {
                continue;
            };
            let again = read_packet(&encoded, &config).unwrap();
            assert_eq!(again, packet, "re-encoded datagram reads differently");
            assert_eq!(write_packet(&again, &config).unwrap(), encoded);
            identical += usize::from(encoded == datagram);
        }
    }
    assert!(
        accepted > 2000 && identical > 1000,
        "{accepted} accepted, {identical} identical"
    );
}

fn text(value: &str) -> ArchiveText {
    ArchiveText::text(value).unwrap()
}

fn messages() -> Vec<ControlMessage> {
    vec![
        ControlMessage::Hello {
            first_field: 1,
            version: 0x0102_0304,
            token: text("LAB"),
        },
        ControlMessage::Challenge {
            token: text("0123"),
        },
        ControlMessage::Login {
            response: text(""),
            url: text("?Name=Lab"),
            unique_net_id: UniqueNetId {
                kind: NetIdType::Named(text("EOS")),
                value: NetIdValue::from_hex("FF").unwrap(),
            },
            platform: text("Lab"),
        },
        ControlMessage::Failure {
            message: text("failed"),
        },
        ControlMessage::Netspeed { rate: 10_000 },
        ControlMessage::Join,
    ]
}

fn bodies(codec: &ControlCodec) -> Vec<Vec<u8>> {
    let encoded: Vec<Vec<u8>> = messages()
        .iter()
        .map(|message| encode_control_message(message, codec).unwrap())
        .collect();
    let mut bodies = encoded.clone();
    bodies.push(encoded.concat());
    bodies
}

#[test]
fn control_bodies_never_panic_and_accepted_messages_re_encode() {
    let mut rng = Rng(0xc0de_c0de_0bad_f00d);
    let mut accepted = 0usize;
    for order in [ByteOrder::Little, ByteOrder::Big] {
        let codec = ControlCodec {
            order,
            ..ControlCodec::default()
        };
        let corpus = bodies(&codec);
        for _ in 0..20_000 {
            let body = mutate(&mut rng, &corpus);
            let mut reader = BitReader::new(&body, body.len() * 8).unwrap();
            let Ok(decoded) = read_control_messages(&mut reader, &codec) else {
                continue;
            };
            accepted += 1;
            // An unknown ID stops decoding after its byte and has no writer;
            // every decoded message re-encodes and reads back unchanged.
            for item in &decoded {
                if let Decoded::Message(message) = item {
                    let encoded = encode_control_message(message, &codec).unwrap();
                    let mut again = BitReader::new(&encoded, encoded.len() * 8).unwrap();
                    let read = read_control_messages(&mut again, &codec).unwrap();
                    assert!(
                        matches!(read.as_slice(), [Decoded::Message(read)] if read == message),
                        "re-encoded message reads differently"
                    );
                }
            }
        }
    }
    assert!(accepted > 3000, "only {accepted} accepted bodies");
}
