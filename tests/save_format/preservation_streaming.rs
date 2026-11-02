//! The streaming load verifier behind `PreservedSection::from_native_encoding`
//! accepts exactly what the tree decoder plus reconstruction plus digest check
//! (`PreservedProspect::from_native_encoding`) accepts, with the same summary.
//! Authored bytes only; every name and value is invented here.
use icarus_bevy::{
    experimental::component::MAX_BOUNDED_COMPONENT_BYTES,
    preservation::{PreserveLimits, PreservedProspect, PreservedSection},
};
use sha2::{Digest, Sha256};

const MARKER: &str = "StreamSentinel";

fn digest(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn assert_private(text: &str) {
    assert!(!text.contains(MARKER), "{text}");
}

/// Both verdicts for one encoding and payload identity; panics on divergence.
fn both(encoded: &[u8], payload_bytes: usize, payload_sha256: [u8; 32]) -> bool {
    let tree = PreservedProspect::from_native_encoding(encoded, payload_bytes, payload_sha256)
        .map(|tree| tree.summary().clone());
    let section =
        PreservedSection::from_native_encoding(encoded.to_vec(), payload_bytes, payload_sha256);
    match (&tree, &section) {
        (Ok(expected), Ok(held)) => {
            assert_eq!(held.summary(), expected, "summaries differ");
            assert_eq!(held.payload_bytes(), payload_bytes);
            assert_eq!(held.encoded_sha256(), digest(encoded));
            true
        }
        (Err(_), Err(error)) => {
            assert_private(&format!("{error:#}"));
            assert_private(&format!("{error:?}"));
            false
        }
        _ => panic!(
            "verdicts differ: tree {:?}, streaming {:?}",
            tree.as_ref()
                .map(|_| ())
                .map_err(|error| format!("{error:#}")),
            section
                .as_ref()
                .map(|_| ())
                .map_err(|error| format!("{error:#}")),
        ),
    }
}

/// Deterministic 64-bit LCG (Knuth's MMIX constants).
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn below(&mut self, bound: usize) -> usize {
        usize::try_from(self.next()).unwrap() % bound
    }

    fn byte(&mut self) -> u8 {
        u8::try_from(self.next() & 0xff).unwrap()
    }

    fn bytes(&mut self, max: usize) -> Vec<u8> {
        let length = self.below(max + 1);
        (0..length).map(|_| self.byte()).collect()
    }
}

// ---- Authored inner payloads, preserved through the public constructor ----

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

fn count(value: usize) -> Vec<u8> {
    i32::try_from(value).unwrap().to_le_bytes().to_vec()
}

fn tag(name: &str, kind: &str, extra: &[u8], value: &[u8]) -> Vec<u8> {
    [
        string(name).as_slice(),
        &string(kind),
        &count(value.len()),
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

fn int(name: &str, value: i32) -> Vec<u8> {
    tag(name, "IntProperty", &[], &value.to_le_bytes())
}

/// Ints, a nested struct, a struct array, a Map<Str, Struct> with a removal
/// key and a Set<Int>: every framed value kind and container item kind.
fn rich_body() -> Vec<u8> {
    let field = format!("{MARKER}Field");
    let element = stream(&[int(&field, 9)]);
    let array = [
        count(2),
        tag(
            &field,
            "StructProperty",
            &struct_extra(MARKER),
            &[element.clone(), element].concat(),
        ),
    ]
    .concat();
    let item = stream(&[int("Inner", 5)]);
    let map = tag(
        "Lookup",
        "MapProperty",
        &[string("StrProperty"), string("StructProperty")].concat(),
        &[count(1), string(MARKER), count(1), string("Key"), item].concat(),
    );
    let set = tag(
        "Members",
        "SetProperty",
        &string("IntProperty"),
        &[count(1), count(3), count(2), count(1), count(2)].concat(),
    );
    let nested = tag(
        &field,
        "StructProperty",
        &struct_extra(MARKER),
        &stream(&[
            tag(&field, "StrProperty", &[], &string(MARKER)),
            int(&field, 1),
        ]),
    );
    [
        stream(&[
            int(&field, 7),
            nested,
            tag(&field, "ArrayProperty", &string("StructProperty"), &array),
            map,
            set,
        ]),
        vec![0; 4],
    ]
    .concat()
}

fn recorder(class: &str, body: &[u8], unknown: bool) -> Vec<u8> {
    let mut fields = Vec::new();
    if unknown {
        fields.push(int(MARKER, 5));
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
        &[count(body.len()), body.to_vec()].concat(),
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
        int(MARKER, 2),
        tag(
            "StateRecorderBlobs",
            "ArrayProperty",
            &string("StructProperty"),
            &[count(recorders.len()), shared].concat(),
        ),
    ])
}

/// Framed, framing-rejected and empty bodies plus an unknown field.
fn small_payloads() -> Vec<Vec<u8>> {
    let class = format!("U{MARKER}Class");
    let rejected = [
        stream(&[tag(MARKER, "StrProperty", &[], &string(MARKER))]),
        [1, 0, 0, 0].to_vec(),
        vec![3; 16],
    ]
    .concat();
    vec![
        payload_with(&[recorder(&class, &rich_body(), true)]),
        payload_with(&[
            recorder(&class, &rich_body(), false),
            recorder(&class, &rejected, true),
            recorder(&class, &[], false),
        ]),
    ]
}

fn encode(payload: &[u8]) -> Vec<u8> {
    PreservedProspect::from_inner_payload(payload, PreserveLimits::default())
        .expect("authored payload preserves")
        .to_native_encoding()
        .unwrap()
}

#[test]
fn valid_fixtures_agree_including_every_opaque_reason() {
    let class = format!("U{MARKER}Class");
    let oversized = stream(&[tag(
        MARKER,
        "StrProperty",
        &[],
        &vec![b'Q'; MAX_BOUNDED_COMPONENT_BYTES],
    )]);
    let mut payloads = small_payloads();
    payloads.push(payload_with(&[
        recorder(&class, &rich_body(), false),
        recorder(&class, &oversized, false),
    ]));
    for payload in payloads {
        let encoded = encode(&payload);
        assert!(both(&encoded, payload.len(), digest(&payload)));
        let held = PreservedSection::from_native_encoding(encoded, payload.len(), digest(&payload))
            .unwrap();
        // The full tree stays available on demand and matches the held summary.
        let tree = held.decode().unwrap();
        assert_eq!(tree.to_inner_payload().unwrap(), payload);
        assert_eq!(tree.summary(), held.summary());
        assert_private(&format!("{held:?}"));
    }
}

#[test]
fn every_single_byte_mutation_truncation_and_append_agrees() {
    for payload in small_payloads() {
        let encoded = encode(&payload);
        let (bytes, sha) = (payload.len(), digest(&payload));
        assert!(both(&encoded, bytes, sha));
        for index in 0..encoded.len() {
            for change in [0x01, 0x40, 0x80, 0xff] {
                let mut altered = encoded.clone();
                altered[index] ^= change;
                both(&altered, bytes, sha);
            }
            for value in [0x00, 0x02, 0x05, 0x0e] {
                let mut altered = encoded.clone();
                altered[index] = value;
                both(&altered, bytes, sha);
            }
            assert!(!both(&encoded[..index], bytes, sha));
        }
        for extra in [0x00, 0x01, 0xff] {
            let appended = [encoded.as_slice(), &[extra]].concat();
            assert!(!both(&appended, bytes, sha));
        }
        // The payload identity itself.
        let mut wrong = sha;
        wrong[31] ^= 1;
        assert!(!both(&encoded, bytes, wrong));
        assert!(!both(&encoded, bytes - 1, sha));
        assert!(!both(&encoded, bytes + 1, sha));
    }
}

#[test]
fn every_count_overwrite_agrees() {
    // Every four-byte window replaced by counts near every structural boundary.
    let payload = small_payloads().swap_remove(0);
    let encoded = encode(&payload);
    let (bytes, sha) = (payload.len(), digest(&payload));
    for index in 0..encoded.len().saturating_sub(3) {
        for value in [0_u32, 1, 2, 3, 4, 17, 4097, 1 << 20, u32::MAX] {
            let mut altered = encoded.clone();
            altered[index..index + 4].copy_from_slice(&value.to_le_bytes());
            both(&altered, bytes, sha);
        }
    }
}

// ---- A test-side native encoder that also emits the would-be payload ----

/// An encoding fragment and the payload bytes its reconstruction emits.
#[derive(Default)]
struct Pair {
    encoded: Vec<u8>,
    payload: Vec<u8>,
}

impl Pair {
    fn raw(&mut self, bytes: &[u8]) {
        self.encoded.extend_from_slice(bytes);
    }

    fn u32(&mut self, value: usize) {
        self.raw(&u32::try_from(value).unwrap().to_le_bytes());
    }

    /// A chunk whose bytes are also reconstructed.
    fn chunk(&mut self, bytes: &[u8]) {
        self.u32(bytes.len());
        self.raw(bytes);
        self.payload.extend_from_slice(bytes);
    }

    fn append(&mut self, other: Pair) {
        self.encoded.extend_from_slice(&other.encoded);
        self.payload.extend_from_slice(&other.payload);
    }
}

/// Random structures; with `faults` above zero, rare targeted violations.
struct Gen {
    rng: Lcg,
    /// Out of 1000 per decision point.
    faults: usize,
    max_depth: usize,
}

impl Gen {
    fn fault(&mut self) -> bool {
        self.faults > 0 && self.rng.below(1000) < self.faults
    }

    fn kind(&mut self) -> u8 {
        if self.fault() {
            14 + self.rng.byte() % 3
        } else {
            u8::try_from(self.rng.below(14)).unwrap()
        }
    }

    fn header(&mut self, declared: usize) -> Vec<u8> {
        if self.fault() {
            return self.rng.bytes(12);
        }
        let declared = match self.fault() {
            true if declared > 0 && self.rng.below(2) == 0 => declared - 1,
            true => declared + 1,
            false => declared,
        };
        let name = if self.rng.below(2) == 0 { MARKER } else { "" };
        let name = if name.is_empty() {
            0_i32.to_le_bytes().to_vec()
        } else {
            string(name)
        };
        [name, string("T"), count(declared), vec![0; 4], vec![0]].concat()
    }

    fn fixed(&mut self, bytes: &[u8]) -> Vec<u8> {
        if self.fault() {
            bytes[..self.rng.below(bytes.len())].to_vec()
        } else {
            bytes.to_vec()
        }
    }

    fn code(&mut self, valid: u8, invalid: u8) -> u8 {
        if self.fault() { invalid } else { valid }
    }

    fn tree(&mut self, depth: usize) -> Pair {
        let mut out = Pair::default();
        let nodes = if depth >= self.max_depth {
            0
        } else {
            self.rng.below(4)
        };
        out.u32(nodes);
        for _ in 0..nodes {
            let node = self.node(depth);
            out.append(node);
        }
        out.chunk(&string("None"));
        out
    }

    fn node(&mut self, depth: usize) -> Pair {
        let nest = depth < self.max_depth && self.rng.below(depth + 1) == 0;
        let choice = if nest { self.rng.below(5) } else { 0 };
        let mut value = Pair::default();
        let code = match choice {
            1 => {
                value.append(self.tree(depth + 1));
                1
            }
            2 => {
                self.struct_array(&mut value, depth);
                2
            }
            3 | 4 => {
                self.container(&mut value, depth, choice == 3);
                u8::try_from(choice).unwrap()
            }
            _ => {
                let bytes = self.rng.bytes(6);
                value.chunk(&bytes);
                0
            }
        };
        let mut out = Pair::default();
        out.raw(&[self.kind()]);
        let header = self.header(value.payload.len());
        out.chunk(&header);
        let invalid = 5 + self.rng.byte() % 4;
        out.raw(&[self.code(code, invalid)]);
        out.append(value);
        out
    }

    fn struct_array(&mut self, value: &mut Pair, depth: usize) {
        let elements: Vec<Pair> = (0..self.rng.below(3))
            .map(|_| self.tree(depth + 1))
            .collect();
        let shared: usize = elements.iter().map(|e| e.payload.len()).sum();
        let mut declared = elements.len();
        if self.fault() {
            declared += 1;
        }
        let prefix = self.fixed(&count(declared));
        value.chunk(&prefix);
        let header = self.header(shared);
        value.chunk(&header);
        value.u32(elements.len());
        for element in elements {
            value.append(element);
        }
    }

    fn container(&mut self, value: &mut Pair, depth: usize, map: bool) {
        for group in [1, if map { 2 } else { 1 }] {
            let entries = self.rng.below(3);
            value.u32(entries);
            value.payload.extend_from_slice(&count(entries));
            for _ in 0..entries * group {
                self.item(value, depth);
            }
        }
    }

    fn item(&mut self, value: &mut Pair, depth: usize) {
        if depth < self.max_depth && self.rng.below(3) == 0 {
            value.raw(&[1]);
            value.append(self.tree(depth + 1));
        } else {
            value.raw(&[self.code(0, 2)]);
            let bytes = self.rng.bytes(5);
            value.chunk(&bytes);
        }
    }

    fn body(&mut self) -> Pair {
        let mut out = Pair::default();
        if self.rng.below(3) == 0 {
            let reason = u8::try_from(self.rng.below(3)).unwrap();
            out.raw(&[0, self.code(reason, 3)]);
            let bytes = self.rng.bytes(8);
            out.chunk(&bytes);
        } else {
            out.raw(&[self.code(1, 2)]);
            out.append(self.tree(1));
            let trailer = self.fixed(&[0, 0, 0, 0]);
            out.chunk(&trailer);
        }
        out
    }

    fn field(&mut self, role: u8) -> Pair {
        let mut out = Pair::default();
        out.raw(&[self.code(role, 3), self.kind()]);
        let header = self.rng.bytes(6);
        out.chunk(&header);
        if role == 2 {
            let prefix = self.fixed(&[1, 0, 0, 0]);
            out.chunk(&prefix);
            out.append(self.body());
        } else {
            let bytes = self.rng.bytes(6);
            out.chunk(&bytes);
        }
        out
    }

    fn record(&mut self) -> Pair {
        let mut roles = vec![1, 2];
        roles.extend(std::iter::repeat_n(0, self.rng.below(3)));
        if self.fault() {
            roles.push(u8::try_from(1 + self.rng.below(2)).unwrap());
        }
        if self.fault() {
            roles.remove(self.rng.below(roles.len()));
        }
        // Original field order is free; shuffle deterministically.
        for index in (1..roles.len()).rev() {
            roles.swap(index, self.rng.below(index + 1));
        }
        let mut out = Pair::default();
        out.u32(roles.len());
        for role in roles {
            let field = self.field(role);
            out.append(field);
        }
        out.chunk(&string("None"));
        out
    }

    fn recorders(&mut self) -> Pair {
        let mut out = Pair::default();
        out.raw(&[1]);
        let header = self.rng.bytes(8);
        out.chunk(&header);
        let prefix = self.fixed(&[2, 0, 0, 0]);
        out.chunk(&prefix);
        let shared = self.rng.bytes(8);
        out.chunk(&shared);
        let records = self.rng.below(4);
        out.u32(records);
        for _ in 0..records {
            let record = self.record();
            out.append(record);
        }
        out
    }

    fn opaque_root(&mut self) -> Pair {
        let mut out = Pair::default();
        out.raw(&[self.code(0, 2), self.kind()]);
        let header = self.rng.bytes(6);
        out.chunk(&header);
        let payload = self.rng.bytes(6);
        out.chunk(&payload);
        out
    }

    /// A section and its would-be reconstructed payload.
    fn section(&mut self) -> Pair {
        let mut roots: Vec<Pair> = (0..self.rng.below(3)).map(|_| self.opaque_root()).collect();
        let recorder_roots = match self.fault() {
            true => 2 * self.rng.below(2),
            false => 1,
        };
        for _ in 0..recorder_roots {
            let at = self.rng.below(roots.len() + 1);
            let root = self.recorders();
            roots.insert(at, root);
        }
        let terminator = string("None");
        let mut out = Pair::default();
        out.u32(terminator.len());
        out.raw(&terminator);
        out.u32(roots.len());
        for root in roots {
            out.append(root);
        }
        out.payload.extend_from_slice(&terminator);
        out
    }
}

fn mutate(rng: &mut Lcg, encoded: &[u8]) -> Vec<u8> {
    let mut altered = encoded.to_vec();
    match rng.below(4) {
        0 if !altered.is_empty() => {
            let index = rng.below(altered.len());
            altered[index] ^= 1 << rng.below(8);
        }
        1 => altered.truncate(rng.below(altered.len() + 1)),
        2 if altered.len() >= 4 => {
            let index = rng.below(altered.len() - 3);
            let value = u32::try_from(rng.below(40)).unwrap();
            altered[index..index + 4].copy_from_slice(&value.to_le_bytes());
        }
        _ => altered.push(rng.byte()),
    }
    altered
}

#[test]
fn seeded_random_structures_and_violations_agree() {
    let mut rng = Lcg(0x5eed_0f57_ea00_u64);
    let (mut accepted, mut rejected) = (0, 0);
    for sample in 0..4000 {
        let faults = if sample % 2 == 0 { 0 } else { 40 };
        // Odd faulty samples may also reach past the depth ceiling.
        let max_depth = if faults > 0 && sample % 7 == 1 {
            18
        } else {
            16
        };
        let mut generator = Gen {
            rng: Lcg(rng.next()),
            faults,
            max_depth,
        };
        let pair = generator.section();
        let (bytes, sha) = (pair.payload.len(), digest(&pair.payload));
        let verdict = both(&pair.encoded, bytes, sha);
        if faults == 0 {
            assert!(verdict, "fault-free sample {sample} must be accepted");
        }
        if verdict {
            accepted += 1;
        } else {
            rejected += 1;
        }
        for _ in 0..4 {
            let altered = mutate(&mut rng, &pair.encoded);
            both(&altered, bytes, sha);
        }
    }
    assert!(accepted > 2000 && rejected > 300, "{accepted} {rejected}");
}

// ---- Crafted ceilings, exactly at and just past each limit ----

/// A minimal record: an empty class field and an empty opaque binary body.
fn minimal_record(extra_unknown: usize) -> Pair {
    let mut out = Pair::default();
    out.u32(2 + extra_unknown);
    for _ in 0..extra_unknown {
        out.raw(&[0, 4]);
        out.chunk(&[]);
        out.chunk(&[]);
    }
    out.raw(&[1, 2]);
    out.chunk(&[]);
    out.chunk(&[]);
    out.raw(&[2, 1]);
    out.chunk(&[]);
    out.chunk(&[0; 4]);
    out.raw(&[0, 1]);
    out.chunk(&[]);
    // The record terminator.
    out.chunk(&[]);
    out
}

fn section_of(opaque_roots: usize, records: &[Pair], body_tree: Option<Pair>) -> Pair {
    let mut out = Pair::default();
    out.u32(0);
    out.u32(opaque_roots + 1);
    for _ in 0..opaque_roots {
        out.raw(&[0, 0]);
        out.chunk(&[]);
        out.chunk(&[]);
    }
    out.raw(&[1]);
    out.chunk(&[]);
    out.chunk(&[0; 4]);
    out.chunk(&[]);
    let framed = usize::from(body_tree.is_some());
    out.u32(records.len() + framed);
    for record in records {
        out.encoded.extend_from_slice(&record.encoded);
        out.payload.extend_from_slice(&record.payload);
    }
    if let Some(tree) = body_tree {
        out.u32(2);
        out.raw(&[1, 2]);
        out.chunk(&[]);
        out.chunk(&[]);
        out.raw(&[2, 1]);
        out.chunk(&[]);
        out.chunk(&[0; 4]);
        out.raw(&[1]);
        out.append(tree);
        out.chunk(&[0; 4]);
        out.chunk(&[]);
    }
    out
}

fn verdict(pair: &Pair) -> bool {
    both(&pair.encoded, pair.payload.len(), digest(&pair.payload))
}

fn property_header(declared: usize) -> Vec<u8> {
    [count(0), count(0), count(declared)].concat()
}

/// A body tree of one Set node holding `items` empty byte items.
fn set_tree(items: usize) -> Pair {
    let mut out = Pair::default();
    out.u32(1);
    out.raw(&[12]);
    out.chunk(&property_header(8));
    out.raw(&[4]);
    for group in [0, items] {
        out.u32(group);
        out.payload.extend_from_slice(&count(group));
        for _ in 0..group {
            out.raw(&[0]);
            out.chunk(&[]);
        }
    }
    out.chunk(&[]);
    out
}

/// Struct nodes nested `depth` trees deep, innermost empty.
fn nested_tree(depth: usize) -> Pair {
    let mut inner = Pair::default();
    inner.u32(0);
    inner.chunk(&[]);
    for _ in 1..depth {
        let mut out = Pair::default();
        out.u32(1);
        out.raw(&[1]);
        out.chunk(&property_header(inner.payload.len()));
        out.raw(&[1]);
        out.append(inner);
        out.chunk(&[]);
        inner = out;
    }
    inner
}

#[test]
fn crafted_ceilings_agree_at_and_past_each_limit() {
    // Roots: 4096 in total, one of them the recorder root.
    assert!(verdict(&section_of(4095, &[minimal_record(0)], None)));
    assert!(!verdict(&section_of(4096, &[minimal_record(0)], None)));
    // Records: 16,384.
    let records: Vec<Pair> = (0..16_384).map(|_| minimal_record(0)).collect();
    assert!(verdict(&section_of(0, &records, None)));
    let records: Vec<Pair> = (0..16_385).map(|_| minimal_record(0)).collect();
    assert!(!verdict(&section_of(0, &records, None)));
    // Recorder fields across the section: 65,536, split across records.
    let split = [minimal_record(30_000), minimal_record(35_532)];
    assert!(verdict(&section_of(0, &split, None)));
    let split = [minimal_record(30_000), minimal_record(35_533)];
    assert!(!verdict(&section_of(0, &split, None)));
    // Depth: 16 nested trees.
    assert!(verdict(&section_of(0, &[], Some(nested_tree(16)))));
    assert!(!verdict(&section_of(0, &[], Some(nested_tree(17)))));
    // Nodes and items per body: 1,048,576, counting the Set node itself.
    assert!(verdict(&section_of(0, &[], Some(set_tree((1 << 20) - 1)))));
    assert!(!verdict(&section_of(0, &[], Some(set_tree(1 << 20)))));
}

#[test]
fn writer_checks_reject_identically_when_the_digest_matches() {
    // Each case's digest is of its would-be reconstruction, so only the
    // writer check itself can reject it.
    let mut opaque = Pair::default();
    opaque.u32(1);
    opaque.raw(&[4]);
    opaque.chunk(&property_header(3));
    opaque.raw(&[0]);
    opaque.chunk(&[1, 2, 3]);
    opaque.chunk(&[]);
    assert!(verdict(&section_of(0, &[], Some(opaque))));
    for declared in [2, 4] {
        let mut wrong = Pair::default();
        wrong.u32(1);
        wrong.raw(&[4]);
        wrong.chunk(&property_header(declared));
        wrong.raw(&[0]);
        wrong.chunk(&[1, 2, 3]);
        wrong.chunk(&[]);
        assert!(!verdict(&section_of(0, &[], Some(wrong))));
    }
    // Struct arrays: count prefix and shared declared size.
    // Two empty element streams, each with a four-byte terminator.
    let element = || {
        let mut tree = Pair::default();
        tree.u32(0);
        tree.chunk(&[0; 4]);
        tree
    };
    let array = |prefix: i32, shared: usize| {
        let mut value = Pair::default();
        value.chunk(&prefix.to_le_bytes());
        value.chunk(&property_header(shared));
        value.u32(2);
        value.append(element());
        value.append(element());
        let mut out = Pair::default();
        out.u32(1);
        out.raw(&[0]);
        out.chunk(&property_header(value.payload.len()));
        out.raw(&[2]);
        out.append(value);
        out.chunk(&[]);
        out
    };
    assert!(verdict(&section_of(0, &[], Some(array(2, 8)))));
    for (prefix, shared) in [(1, 8), (3, 8), (-2, 8), (2, 7), (2, 9)] {
        assert!(!verdict(&section_of(0, &[], Some(array(prefix, shared)))));
    }
    // Malformed headers whose declared size cannot be read.
    for header in [
        vec![],
        vec![0; 11],
        [(-1_i32).to_le_bytes().to_vec(), count(0), count(0)].concat(),
    ] {
        let mut out = Pair::default();
        out.u32(1);
        out.raw(&[4]);
        out.chunk(&header);
        out.raw(&[0]);
        out.chunk(&[]);
        out.chunk(&[]);
        assert!(!verdict(&section_of(0, &[], Some(out))));
    }
    // Recorder composition: no recorder root, and records missing a role.
    let mut none = Pair::default();
    none.u32(0);
    none.u32(1);
    none.raw(&[0, 0]);
    none.chunk(&[]);
    none.chunk(&[]);
    assert!(!verdict(&none));
    let mut lonely = Pair::default();
    lonely.u32(1);
    lonely.raw(&[1, 2]);
    lonely.chunk(&[]);
    lonely.chunk(&[]);
    lonely.chunk(&[]);
    assert!(!verdict(&section_of(0, &[lonely], None)));
}
