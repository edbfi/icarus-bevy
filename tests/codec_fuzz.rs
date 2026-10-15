//! Deterministic randomized robustness checks for the experimental replication
//! codecs: no input panics, and every accepted input re-encodes to exactly the
//! bits it consumed. Inputs are random bit strings and bit-level mutations of
//! valid encodings built from authored values; no captured traffic.

use icarus_bevy::experimental::actor_channel::{
    ActorBunchBody, ActorChannelCodec, ActorChannelLimits, BlockTarget, CompressedRotator,
    ContentBlock, GuidExport, NetGuidRef, read_actor_bunch_body, write_actor_bunch_body,
};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::control::TokenEncoding;
use icarus_bevy::experimental::control_messages::ArchiveText;
use icarus_bevy::experimental::packet::{BitWriter, Payload};
use icarus_bevy::experimental::rep_layout::{
    ByteRotator, GameplayTag, HitResult, MovementLevel, MovementRotation, NetworkName,
    PropertyStream, QuantizedNormal, QuantizedVector, RepArray, RepCommand, RepEntry,
    RepLayoutCodec, RepLayoutLimits, RepMovement, RepValue, RotationLevel, TagEncoding,
    encode_property_stream, read_property_stream, write_property_stream,
};

/// xorshift64*: a fixed-seed generator, so every run checks the same inputs.
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

fn bits_of(payload: &Payload) -> Vec<bool> {
    let mut reader = payload.reader();
    (0..payload.bit_len()).map(|_| reader.read_bit()).collect()
}

fn payload_of(bits: &[bool]) -> Payload {
    let mut writer = BitWriter::new();
    for bit in bits {
        writer.write_bit(*bit);
    }
    Payload::from_writer(writer)
}

/// Random bits, or a valid encoding with flips, a truncation or a tail.
fn mutate(rng: &mut Rng, corpus: &[Vec<bool>]) -> Vec<bool> {
    if rng.below(4) == 0 {
        let len = rng.below(640);
        return (0..len).map(|_| rng.next() & 1 == 1).collect();
    }
    let mut bits = corpus[rng.below(corpus.len())].clone();
    match rng.below(4) {
        0 if !bits.is_empty() => {
            let at = rng.below(bits.len());
            bits.truncate(at);
        }
        1 => {
            for _ in 0..rng.below(16) {
                bits.push(rng.next() & 1 == 1);
            }
        }
        _ => {}
    }
    for _ in 0..=rng.below(3) {
        if !bits.is_empty() {
            let at = rng.below(bits.len());
            bits[at] = !bits[at];
        }
    }
    bits
}

fn actor_codec(order: ByteOrder, exports: bool) -> ActorChannelCodec {
    ActorChannelCodec {
        engine_net_version: 13,
        exports_enabled: exports,
        has_level: false,
        order,
        limits: ActorChannelLimits::default(),
    }
}

fn rep_codec(order: ByteOrder, checksums: bool, exports: bool) -> RepLayoutCodec {
    RepLayoutCodec {
        actor: actor_codec(order, exports),
        checksums,
        limits: RepLayoutLimits::default(),
    }
}

fn narrow(value: &str) -> ArchiveText {
    let units: Vec<u16> = value.bytes().map(u16::from).collect();
    ArchiveText::with_encoding(&units, TokenEncoding::Narrow).unwrap()
}

const VECTOR: QuantizedVector = QuantizedVector {
    bits: 4,
    components: [1, 33, 63],
};

fn layout() -> Vec<RepCommand> {
    vec![
        RepCommand::Bool,
        RepCommand::Byte { enum_bits: Some(3) },
        RepCommand::Enum { bits: 9 },
        RepCommand::Object,
        RepCommand::NetworkName,
        RepCommand::Str,
        RepCommand::SoftObject,
        RepCommand::VectorQuantize,
        RepCommand::VectorQuantize10,
        RepCommand::VectorQuantizeNormal,
        RepCommand::Rotator,
        RepCommand::GameplayTag(TagEncoding::NetIndex {
            first_segment: 3,
            total_bits: 9,
        }),
        RepCommand::GameplayTagContainer {
            encoding: TagEncoding::Name,
            count_bits: 4,
        },
        RepCommand::HitResult,
        RepCommand::Movement {
            location: MovementLevel::Whole,
            velocity: MovementLevel::Tenths,
            rotation: RotationLevel::Bytes,
        },
        RepCommand::DynamicArray {
            element: vec![
                RepCommand::Bool,
                RepCommand::DynamicArray {
                    element: vec![RepCommand::Byte { enum_bits: None }],
                },
            ],
        },
        RepCommand::Opaque { bits: 7 },
    ]
}

fn entry(handle: u32, value: RepValue, checksums: bool) -> RepEntry {
    RepEntry {
        handle,
        checksum: checksums.then_some(handle.wrapping_mul(0x9e37_79b9)),
        value,
    }
}

fn stream(entries: Vec<RepEntry>, checksums: bool) -> PropertyStream {
    PropertyStream {
        entries,
        terminator_checksum: checksums.then_some(0x0bad_cafe),
    }
}

/// A nonzero GUID carries export flags exactly when exports are enabled.
fn guid(value: u32, exports: bool) -> NetGuidRef {
    NetGuidRef {
        guid: value,
        export: (exports && value != 0).then_some(GuidExport {
            no_load: value.is_multiple_of(2),
            path: None,
            checksum: None,
        }),
    }
}

/// Valid streams over [`layout`], encoded with `codec`.
fn property_corpus(codec: &RepLayoutCodec) -> Vec<Vec<bool>> {
    let c = codec.checksums;
    let x = codec.actor.exports_enabled;
    let hit = HitResult {
        flags: 0b0100_0101,
        time: 7,
        location: VECTOR,
        normal: QuantizedNormal([1, 2, 3]),
        impact_point: None,
        impact_normal: Some(QuantizedNormal([4, 5, 6])),
        trace_start: VECTOR,
        trace_end: VECTOR,
        penetration_depth: None,
        item: Some(3),
        phys_material: guid(2, x),
        actor: guid(5, x),
        component: guid(0, x),
        bone_name: NetworkName::Hardcoded(9),
        face_index: Some(1),
        element_index: Some(2),
    };
    let movement = RepMovement {
        flags: 2,
        location: VECTOR,
        rotation: Some(MovementRotation::Bytes(ByteRotator {
            pitch: Some(3),
            yaw: None,
            roll: Some(200),
        })),
        linear_velocity: VECTOR,
        angular_velocity: Some(VECTOR),
    };
    let inner = |count, entries| {
        RepValue::DynamicArray(RepArray {
            count,
            stream: stream(entries, c),
        })
    };
    let streams = [
        stream(Vec::new(), c),
        stream(
            vec![
                entry(1, RepValue::Bool(true), c),
                entry(2, RepValue::Byte(5), c),
                entry(3, RepValue::Enum(300), c),
                entry(4, RepValue::Object(guid(41, x)), c),
                entry(5, RepValue::NetworkName(NetworkName::Hardcoded(255)), c),
                entry(6, RepValue::Str(narrow("abc")), c),
                entry(7, RepValue::SoftObject(narrow("/a")), c),
            ],
            c,
        ),
        stream(
            vec![
                entry(8, RepValue::VectorQuantize(VECTOR), c),
                entry(9, RepValue::VectorQuantize10(VECTOR), c),
                entry(
                    10,
                    RepValue::VectorQuantizeNormal(QuantizedNormal([9, 8, 7])),
                    c,
                ),
                entry(
                    11,
                    RepValue::Rotator(CompressedRotator {
                        pitch: Some(1),
                        yaw: None,
                        roll: Some(65_535),
                    }),
                    c,
                ),
                entry(12, RepValue::GameplayTag(GameplayTag::NetIndex(300)), c),
                entry(
                    13,
                    RepValue::GameplayTagContainer(Some(vec![GameplayTag::Name(
                        NetworkName::Hardcoded(4),
                    )])),
                    c,
                ),
            ],
            c,
        ),
        stream(
            vec![
                entry(14, RepValue::HitResult(Box::new(hit)), c),
                entry(15, RepValue::Movement(movement), c),
            ],
            c,
        ),
        stream(
            vec![
                entry(
                    16,
                    inner(
                        2,
                        vec![
                            entry(1, RepValue::Bool(false), c),
                            entry(4, inner(1, vec![entry(1, RepValue::Byte(200), c)]), c),
                        ],
                    ),
                    c,
                ),
                entry(17, RepValue::Opaque(payload_of(&[true; 7])), c),
            ],
            c,
        ),
    ];
    streams
        .iter()
        .map(|value| bits_of(&encode_property_stream(value, &layout(), codec).unwrap()))
        .collect()
}

#[test]
fn property_streams_never_panic_and_accepted_prefixes_re_encode() {
    let layout = layout();
    let mut rng = Rng(0x1ca7_05b3_ac0d_ec01);
    let mut accepted = 0usize;
    for (order, checksums, exports) in [
        (ByteOrder::Little, false, false),
        (ByteOrder::Big, true, false),
        (ByteOrder::Little, false, true),
    ] {
        let codec = rep_codec(order, checksums, exports);
        let corpus = property_corpus(&codec);
        for _ in 0..20_000 {
            let bits = mutate(&mut rng, &corpus);
            let payload = payload_of(&bits);
            let mut reader = payload.reader();
            let Ok(value) = read_property_stream(&mut reader, &layout, &codec) else {
                continue;
            };
            accepted += 1;
            let consumed = bits.len() - reader.remaining();
            let mut writer = BitWriter::new();
            write_property_stream(&mut writer, &value, &layout, &codec).unwrap();
            assert_eq!(
                bits_of(&Payload::from_writer(writer)),
                bits[..consumed],
                "accepted stream did not re-encode"
            );
        }
    }
    // Mutations keep some inputs valid, so the identity check is exercised.
    assert!(accepted > 2000, "only {accepted} accepted inputs");
}

fn actor_corpus(codec: &ActorChannelCodec) -> Vec<Vec<bool>> {
    let block = |target, has_rep_layout, bits: usize| ContentBlock {
        has_rep_layout,
        target,
        payload: payload_of(&vec![true; bits]),
    };
    let bodies = [
        ActorBunchBody {
            spawn: None,
            blocks: vec![block(BlockTarget::Actor, true, 9)],
        },
        ActorBunchBody {
            spawn: None,
            blocks: vec![
                block(BlockTarget::Actor, false, 0),
                block(BlockTarget::Actor, true, 3),
            ],
        },
    ];
    bodies
        .iter()
        .map(|body| {
            let mut writer = BitWriter::new();
            write_actor_bunch_body(&mut writer, body, codec).unwrap();
            bits_of(&Payload::from_writer(writer))
        })
        .collect()
}

#[test]
fn actor_bunch_bodies_never_panic_and_accepted_bodies_re_encode() {
    let mut rng = Rng(0x5eed_0fac_7012);
    let mut accepted = 0usize;
    for (order, exports) in [(ByteOrder::Little, false), (ByteOrder::Big, true)] {
        let codec = actor_codec(order, exports);
        let corpus = actor_corpus(&codec);
        for round in 0..20_000u32 {
            let bits = mutate(&mut rng, &corpus);
            let payload = payload_of(&bits);
            let open = round.is_multiple_of(2);
            let mut reader = payload.reader();
            let Ok(body) = read_actor_bunch_body(&mut reader, open, &codec) else {
                continue;
            };
            accepted += 1;
            let mut writer = BitWriter::new();
            write_actor_bunch_body(&mut writer, &body, &codec).unwrap();
            assert_eq!(
                bits_of(&Payload::from_writer(writer)),
                bits,
                "accepted body did not re-encode"
            );
        }
    }
    assert!(accepted > 1000, "only {accepted} accepted inputs");
}
