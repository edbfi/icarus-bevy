//! Deterministic randomized robustness checks for lossless preservation and
//! its native encoding. Authored payloads only; every name and value is
//! invented. Mutated inputs never panic; every accepted payload reconstructs
//! byte-identically; and the streaming section verifier accepts exactly the
//! encodings the tree decoder accepts.
use icarus_bevy::preservation::{PreserveLimits, PreservedProspect, PreservedSection};
use sha2::{Digest, Sha256};

const CLASS: &str = "UFuzzSentinelClassQQ";
const OTHER_CLASS: &str = "AFuzzSentinelOtherQQ";
const FIELD: &str = "FuzzSentinelFieldQQ";
const VALUE: &str = "FuzzSentinelValueQQ";

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

// Authored framing, independent of the readers.
fn string(value: &str) -> Vec<u8> {
    [
        i32::try_from(value.len() + 1)
            .unwrap()
            .to_le_bytes()
            .as_slice(),
        value.as_bytes(),
        &[0],
    ]
    .concat()
}

fn tag(name: &str, kind: &str, extra: &[u8], value: &[u8]) -> Vec<u8> {
    [
        string(name).as_slice(),
        &string(kind),
        &i32::try_from(value.len()).unwrap().to_le_bytes(),
        &[0; 4],
        extra,
        &[0],
        value,
    ]
    .concat()
}

fn struct_extra(name: &str) -> Vec<u8> {
    [string(name), vec![7; 16]].concat()
}

fn stream(fields: &[Vec<u8>]) -> Vec<u8> {
    [fields.concat(), string("None")].concat()
}

fn component(fields: &[Vec<u8>], trailer: &[u8]) -> Vec<u8> {
    [stream(fields), trailer.to_vec()].concat()
}

fn framed_body() -> Vec<u8> {
    let element = stream(&[tag(FIELD, "IntProperty", &[], &9_i32.to_le_bytes())]);
    let elements = [element.clone(), element].concat();
    let array = [
        2_i32.to_le_bytes().as_slice(),
        &tag(FIELD, "StructProperty", &struct_extra(CLASS), &elements),
    ]
    .concat();
    component(
        &[
            tag(FIELD, "IntProperty", &[], &7_i32.to_le_bytes()),
            tag(
                FIELD,
                "StructProperty",
                &struct_extra(CLASS),
                &stream(&[tag(FIELD, "StrProperty", &[], &string(VALUE))]),
            ),
            tag(FIELD, "ArrayProperty", &string("StructProperty"), &array),
        ],
        &[0; 4],
    )
}

fn recorder(class: &str, body: &[u8], unknown: bool) -> Vec<u8> {
    let mut fields = Vec::new();
    if unknown {
        fields.push(tag(FIELD, "IntProperty", &[], &5_i32.to_le_bytes()));
    }
    fields.push(tag(
        "ComponentClassName",
        "StrProperty",
        &[],
        &string(class),
    ));
    fields.push(tag(
        "BinaryData",
        "ArrayProperty",
        &string("ByteProperty"),
        &[
            i32::try_from(body.len()).unwrap().to_le_bytes().as_slice(),
            body,
        ]
        .concat(),
    ));
    stream(&fields)
}

fn payload_with(recorders: &[Vec<u8>]) -> Vec<u8> {
    let shared = tag(
        "StateRecorderBlobs",
        "StructProperty",
        &struct_extra("StateRecorderBlob"),
        &recorders.concat(),
    );
    stream(&[
        tag(FIELD, "IntProperty", &[], &2_i32.to_le_bytes()),
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &string("StructProperty"),
            &[
                i32::try_from(recorders.len())
                    .unwrap()
                    .to_le_bytes()
                    .as_slice(),
                &shared,
            ]
            .concat(),
        ),
    ])
}

fn payload() -> Vec<u8> {
    let rejected = component(
        &[tag(FIELD, "StrProperty", &[], &string(VALUE))],
        &[[1, 0, 0, 0].as_slice(), &[3; 16]].concat(),
    );
    payload_with(&[
        recorder(CLASS, &framed_body(), true),
        recorder(OTHER_CLASS, &rejected, false),
        recorder(OTHER_CLASS, &[], false),
    ])
}

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn mutate(rng: &mut Rng, base: &[u8]) -> Vec<u8> {
    let mut bytes = base.to_vec();
    match rng.below(5) {
        0 => {
            let at = rng.below(bytes.len() + 1);
            bytes.truncate(at);
        }
        1 => {
            for _ in 0..=rng.below(8) {
                bytes.push(rng.next() as u8);
            }
        }
        2 if !bytes.is_empty() => {
            // A plausible small length or count over any four aligned bytes.
            let at = rng.below(bytes.len());
            let value = (rng.below(64) as i32 - 8).to_le_bytes();
            for (offset, byte) in value.iter().enumerate() {
                if let Some(slot) = bytes.get_mut(at + offset) {
                    *slot = *byte;
                }
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

#[test]
fn mutated_payloads_never_panic_and_accepted_ones_reconstruct_exactly() {
    let base = payload();
    let mut rng = Rng(0x0f0f_1e55_c0de_d00d);
    let mut accepted = 0usize;
    for _ in 0..4000 {
        let input = mutate(&mut rng, &base);
        let Ok(tree) = PreservedProspect::from_inner_payload(&input, PreserveLimits::default())
        else {
            continue;
        };
        accepted += 1;
        assert_eq!(tree.to_inner_payload().unwrap(), input);
        let encoded = tree.to_native_encoding().unwrap();
        let decoded =
            PreservedProspect::from_native_encoding(&encoded, input.len(), digest(&input)).unwrap();
        assert_eq!(decoded.to_inner_payload().unwrap(), input);
        let section =
            PreservedSection::from_native_encoding(encoded.clone(), input.len(), digest(&input))
                .unwrap();
        assert_eq!(section.summary(), tree.summary());
        assert_eq!(
            section.decode().unwrap().to_native_encoding().unwrap(),
            encoded
        );
    }
    assert!(accepted > 300, "only {accepted} accepted payloads");
}

#[test]
fn the_streaming_verifier_accepts_exactly_what_the_decoder_accepts() {
    let base = payload();
    let tree = PreservedProspect::from_inner_payload(&base, PreserveLimits::default()).unwrap();
    let encoded = tree.to_native_encoding().unwrap();
    let mut rng = Rng(0xdec0_de5e_c710_0001);
    let (mut both_ok, mut both_err) = (0usize, 0usize);
    for round in 0..6000u32 {
        let input = if round.is_multiple_of(10) {
            encoded.clone()
        } else {
            mutate(&mut rng, &encoded)
        };
        // Every seventh round expects a payload digest nothing reconstructs.
        let (bytes, sha) = if round.is_multiple_of(7) {
            (base.len(), [0; 32])
        } else {
            (base.len(), digest(&base))
        };
        let decoder = PreservedProspect::from_native_encoding(&input, bytes, sha);
        let verifier = PreservedSection::from_native_encoding(input.clone(), bytes, sha);
        assert_eq!(
            decoder.is_ok(),
            verifier.is_ok(),
            "decoder and streaming verifier disagree"
        );
        if let (Ok(decoder), Ok(verifier)) = (&decoder, &verifier) {
            assert_eq!(verifier.summary(), decoder.summary());
            both_ok += 1;
        } else {
            both_err += 1;
        }
    }
    assert!(
        both_ok > 400 && both_err > 1000,
        "{both_ok} ok, {both_err} err"
    );
}
