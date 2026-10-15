//! Deterministic randomized corruption of a committed snapshot store: random
//! bit flips, truncations, extensions and emptied files in the manifest, either
//! generation or a preserved-section sidecar. Opening and loading never panic,
//! never report an empty world, and return either a committed snapshot exactly
//! (the current one unless recovery is reported) or an error. Authored payloads
//! only; every name and value is invented.
use std::{collections::BTreeMap, fs, sync::Arc};

use icarus_bevy::{
    persistence::SnapshotStore,
    preservation::{PreserveLimits, PreservedSection},
    simulation::{DomainId, EntityRecord, Position, WorldSnapshot},
};

const CLASS: &str = "UStoreFuzzSentinelQQ";
const FIELD: &str = "StoreFuzzFieldQQ";

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

fn stream(fields: &[Vec<u8>]) -> Vec<u8> {
    [fields.concat(), string("None")].concat()
}

fn recorder(value: i32) -> Vec<u8> {
    let body = [
        stream(&[tag(FIELD, "IntProperty", &[], &value.to_le_bytes())]),
        vec![0; 4],
    ]
    .concat();
    stream(&[
        tag("ComponentClassName", "StrProperty", &[], &string(CLASS)),
        tag(
            "BinaryData",
            "ArrayProperty",
            &string("ByteProperty"),
            &[
                i32::try_from(body.len()).unwrap().to_le_bytes().as_slice(),
                &body,
            ]
            .concat(),
        ),
    ])
}

fn payload(value: i32) -> Vec<u8> {
    let records = [recorder(value), recorder(value + 1)];
    let shared = tag(
        "StateRecorderBlobs",
        "StructProperty",
        &[string("StateRecorderBlob"), vec![7; 16]].concat(),
        &records.concat(),
    );
    stream(&[tag(
        "StateRecorderBlobs",
        "ArrayProperty",
        &string("StructProperty"),
        &[2_i32.to_le_bytes().as_slice(), &shared].concat(),
    )])
}

fn snapshot(tick: u64, value: i32) -> WorldSnapshot {
    let section =
        PreservedSection::from_inner_payload(&payload(value), PreserveLimits::default()).unwrap();
    WorldSnapshot {
        schema_version: 3,
        tick,
        entities: vec![EntityRecord {
            id: DomainId(7),
            position: Position([1.0, 2.0, tick as f32]),
        }],
        preserved_prospect: Some(Arc::new(section)),
    }
}

fn mutate(rng: &mut Rng, bytes: &mut Vec<u8>) {
    match rng.below(6) {
        0 => bytes.clear(),
        1 => {
            let at = rng.below(bytes.len() + 1);
            bytes.truncate(at);
        }
        2 => {
            for _ in 0..=rng.below(8) {
                bytes.push(rng.next() as u8);
            }
        }
        _ => {
            for _ in 0..=rng.below(4) {
                if !bytes.is_empty() {
                    let at = rng.below(bytes.len());
                    bytes[at] ^= 1 << rng.below(8);
                }
            }
        }
    }
}

#[test]
fn corrupted_stores_load_a_committed_snapshot_or_fail_closed() {
    let previous = snapshot(10, 5);
    let current = snapshot(20, 9);
    let pristine = tempfile::tempdir().unwrap();
    {
        let mut store = SnapshotStore::open(pristine.path()).unwrap();
        store.save(&previous).unwrap();
        store.save(&current).unwrap();
    }
    let files: BTreeMap<String, Vec<u8>> = fs::read_dir(pristine.path())
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_file())
        .map(|entry| {
            (
                entry.file_name().into_string().unwrap(),
                fs::read(entry.path()).unwrap(),
            )
        })
        .filter(|(name, _)| !name.ends_with(".lock"))
        .collect();
    let targets: Vec<String> = files
        .keys()
        .filter(|name| {
            *name == "manifest.json"
                || name.starts_with("snapshot-")
                || name.starts_with("preserved-")
        })
        .cloned()
        .collect();
    assert!(targets.len() >= 5, "{targets:?}");

    let mut rng = Rng(0x5701_e5ee_d0fa_1175);
    let (mut current_loads, mut recoveries, mut failures) = (0usize, 0usize, 0usize);
    for _ in 0..1500 {
        let directory = tempfile::tempdir().unwrap();
        let target = &targets[rng.below(targets.len())];
        for (name, bytes) in &files {
            let mut bytes = bytes.clone();
            if name == target {
                mutate(&mut rng, &mut bytes);
            }
            fs::write(directory.path().join(name), bytes).unwrap();
        }
        let loaded = SnapshotStore::open(directory.path()).and_then(|store| store.load());
        match loaded {
            Ok(Some(loaded)) if loaded.recovered_previous => {
                assert_eq!(loaded.snapshot, previous, "recovered a wrong snapshot");
                recoveries += 1;
            }
            Ok(Some(loaded)) => {
                assert_eq!(loaded.snapshot, current, "loaded a wrong snapshot");
                current_loads += 1;
            }
            Ok(None) => panic!("a corrupted committed store loaded as an empty world"),
            Err(_) => failures += 1,
        }
    }
    assert!(
        current_loads > 100 && recoveries > 100 && failures > 100,
        "{current_loads} current, {recoveries} recovered, {failures} failed"
    );
}
