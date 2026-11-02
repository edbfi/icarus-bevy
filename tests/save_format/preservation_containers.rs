//! Map and Set container nodes survive lossless preservation and its native
//! encoding. Authored bytes only; the sentinel strings must never be reported.
use icarus_bevy::preservation::{PreserveLimits, PreservedProspect};

const SECRET: &[u8] = b"ContainerSentinelQQ";

fn count(value: usize) -> Vec<u8> {
    i32::try_from(value).unwrap().to_le_bytes().to_vec()
}

fn string(value: &[u8]) -> Vec<u8> {
    [count(value.len() + 1).as_slice(), value, &[0]].concat()
}

fn tag(name: &[u8], kind: &[u8], extra: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        string(name).as_slice(),
        &string(kind),
        &count(payload.len()),
        &[0; 4],
        extra,
        &[0],
        payload,
    ]
    .concat()
}

/// One recorder whose component body holds a Map<Str, Struct> and a Set<Int>.
fn payload() -> Vec<u8> {
    let item_stream = [
        tag(b"Inner", b"IntProperty", &[], &[5, 0, 0, 0]),
        string(b"None"),
    ]
    .concat();
    let map = tag(
        b"Lookup",
        b"MapProperty",
        &[string(b"StrProperty"), string(b"StructProperty")].concat(),
        &[count(0), count(1), string(SECRET), item_stream].concat(),
    );
    let set = tag(
        b"Members",
        b"SetProperty",
        &string(b"IntProperty"),
        &[count(0), count(2), vec![1, 0, 0, 0], vec![2, 0, 0, 0]].concat(),
    );
    let body = [map, set, string(b"None"), vec![0; 4]].concat();
    let fields = [
        tag(b"ComponentClassName", b"StrProperty", &[], &string(b"Demo")),
        tag(
            b"BinaryData",
            b"ArrayProperty",
            &string(b"ByteProperty"),
            &[count(body.len()), body].concat(),
        ),
        string(b"None"),
    ]
    .concat();
    let shared = tag(
        b"StateRecorderBlobs",
        b"StructProperty",
        &[string(b"StateRecorderBlob"), vec![0; 16]].concat(),
        &fields,
    );
    [
        tag(
            b"StateRecorderBlobs",
            b"ArrayProperty",
            &string(b"StructProperty"),
            &[count(1), shared].concat(),
        ),
        string(b"None"),
    ]
    .concat()
}

#[test]
fn container_nodes_round_trip_through_native_encoding() {
    let payload = payload();
    let preserved = PreservedProspect::from_inner_payload(&payload, PreserveLimits::default())
        .expect("authored payload preserves");
    let summary = preserved.summary();
    assert_eq!(summary.framed_bodies, 1);
    assert_eq!(summary.opaque_bodies, 0);
    // Map, Set and the struct item's inner property: the Map framed into items.
    assert_eq!(summary.framed_property_nodes, 3);
    assert!(summary.byte_identical);
    assert_eq!(preserved.to_inner_payload().unwrap(), payload);

    let encoded = preserved.to_native_encoding().unwrap();
    let decoded = PreservedProspect::from_native_encoding(
        &encoded,
        preserved.payload_bytes(),
        preserved.payload_sha256(),
    )
    .expect("native encoding decodes");
    assert_eq!(decoded.to_inner_payload().unwrap(), payload);
    assert_eq!(decoded.summary(), summary);

    let text = format!("{summary:?}");
    assert!(!text.contains("ContainerSentinel"));
}

#[test]
fn corrupt_container_codes_fail_closed() {
    let payload = payload();
    let preserved =
        PreservedProspect::from_inner_payload(&payload, PreserveLimits::default()).unwrap();
    let encoded = preserved.to_native_encoding().unwrap();
    // Flipping any single byte must never decode to a different payload.
    for index in 0..encoded.len() {
        let mut altered = encoded.clone();
        altered[index] ^= 0x40;
        if let Ok(decoded) = PreservedProspect::from_native_encoding(
            &altered,
            preserved.payload_bytes(),
            preserved.payload_sha256(),
        ) {
            assert_eq!(decoded.to_inner_payload().unwrap(), payload, "byte {index}");
        }
    }
}
