use icarus_bevy::experimental::property_tree::{
    BaseComponentTree, ContainerItem, NodeValue, PropertyTree, TreeLimits,
};
use icarus_bevy::experimental::tagged::{TagKind, TagProfile};

const END: &[u8] = b"\x05\0\0\0None\0";
const ARRAY: &[u8] = b"\x0e\0\0\0ArrayProperty\0";
const STRUCT: &[u8] = b"\x0f\0\0\0StructProperty\0";
const STR: &[u8] = b"\x0c\0\0\0StrProperty\0";
const INT: &[u8] = b"\x0c\0\0\0IntProperty\0";
const NAME: &[u8] = b"\x0d\0\0\0NameProperty\0";
const FLOAT: &[u8] = b"\x0e\0\0\0FloatProperty\0";
const BOOL: &[u8] = b"\x0d\0\0\0BoolProperty\0";
const ENUM: &[u8] = b"\x0d\0\0\0EnumProperty\0";
const SECRET: &[u8] = b"SentinelValueQQ";

// Independently authored header bytes; the sentinel name must never be reported.
fn prop(kind: &[u8], index: i32, extensions: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        b"\x0f\0\0\0SentinelNameQQ\0".as_slice(),
        kind,
        &i32::try_from(payload.len()).unwrap().to_le_bytes(),
        &index.to_le_bytes(),
        extensions,
        payload,
    ]
    .concat()
}

fn struct_prop(payload: &[u8]) -> Vec<u8> {
    let extensions = [b"\x05\0\0\0Demo\0".as_slice(), &[7; 16], &[0]].concat();
    prop(STRUCT, 0, &extensions, payload)
}

fn int(value: u8) -> Vec<u8> {
    prop(INT, 0, &[0], &[value, 0, 0, 0])
}

fn stream(properties: &[Vec<u8>]) -> Vec<u8> {
    [properties.concat().as_slice(), END].concat()
}

/// Count prefix, shared StructProperty header and None-terminated elements.
fn struct_array(elements: &[Vec<u8>]) -> Vec<u8> {
    let count = i32::try_from(elements.len()).unwrap().to_le_bytes();
    let extensions = [b"\x0f\0\0\0StructProperty\0".as_slice(), &[0]].concat();
    prop(
        ARRAY,
        0,
        &extensions,
        &[&count[..], &struct_prop(&elements.concat())].concat(),
    )
}

fn parse(bytes: &[u8]) -> anyhow::Result<PropertyTree> {
    PropertyTree::parse(bytes, TagProfile::MemoryArchive522, TreeLimits::default())
}

fn depth_limited(bytes: &[u8], max_depth: usize) -> anyhow::Result<PropertyTree> {
    let limits = TreeLimits {
        max_depth,
        ..TreeLimits::default()
    };
    PropertyTree::parse(bytes, TagProfile::MemoryArchive522, limits)
}

fn round_trip(bytes: &[u8]) -> PropertyTree {
    let tree = parse(bytes).unwrap();
    assert_eq!(tree.to_bytes().unwrap(), bytes);
    tree
}

fn contains(haystack: &[u8], needle: &[u8]) -> bool {
    haystack
        .windows(needle.len())
        .any(|window| window == needle)
}

#[test]
fn flat_leaves_round_trip_as_exact_opaque_payloads() {
    let guid = [&[1][..], &[9; 16]].concat();
    let leaves = [
        int(42),
        prop(
            STR,
            0,
            &[0],
            &[b"\x10\0\0\0".as_slice(), SECRET, &[0]].concat(),
        ),
        prop(NAME, 3, &guid, b"not an FString"),
        prop(FLOAT, 0, &[0], &[0xff; 4]),
        prop(BOOL, 0, &[1, 0], &[]),
        prop(
            ENUM,
            0,
            &[b"\x02\0\0\0E\0".as_slice(), &[0]].concat(),
            SECRET,
        ),
    ];
    let bytes = stream(&leaves);
    let tree = round_trip(&bytes);
    assert_eq!(tree.properties.len(), leaves.len());
    assert_eq!(tree.terminator, END);
    for (node, original) in tree.properties.iter().zip(&leaves) {
        let NodeValue::Opaque(payload) = &node.value else {
            panic!("leaf should stay opaque");
        };
        assert_eq!([node.header.as_slice(), payload].concat(), *original);
    }
    let kinds: Vec<_> = tree.properties.iter().map(|node| node.kind).collect();
    assert_eq!(
        kinds,
        [
            TagKind::Int,
            TagKind::String,
            TagKind::Name,
            TagKind::Float,
            TagKind::Bool,
            TagKind::Enum
        ]
    );
    // The empty stream is a lone terminator.
    let empty = round_trip(END);
    assert!(empty.properties.is_empty());
    assert_eq!(empty.summary().max_depth, 0);
}

#[test]
fn nested_struct_frames_as_tree_and_binary_payloads_stay_opaque() {
    let inner = stream(&[int(1), prop(STR, 0, &[0], SECRET)]);
    let bytes = stream(&[struct_prop(&inner)]);
    let tree = round_trip(&bytes);
    let NodeValue::Struct(nested) = &tree.properties[0].value else {
        panic!("tagged struct payload should frame");
    };
    assert_eq!(nested.properties.len(), 2);
    assert_eq!(nested.terminator, END);
    assert_eq!(nested.to_bytes().unwrap(), inner);

    // Native/binary payloads, a trailing byte after None and an empty payload
    // fail exact framing, so they are kept verbatim.
    for payload in [
        vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
        [inner.as_slice(), &[0]].concat(),
        [END, SECRET].concat(),
        Vec::new(),
    ] {
        let bytes = stream(&[struct_prop(&payload)]);
        let tree = round_trip(&bytes);
        let NodeValue::Opaque(kept) = &tree.properties[0].value else {
            panic!("binary struct payload should stay opaque");
        };
        assert_eq!(*kept, payload);
    }
}

#[test]
fn struct_arrays_with_zero_one_and_three_elements_round_trip() {
    for count in [0, 1, 3] {
        let elements: Vec<_> = (0..count)
            .map(|i| stream(&[int(i), prop(STR, 0, &[0], SECRET)]))
            .collect();
        let bytes = stream(&[struct_array(&elements)]);
        let tree = round_trip(&bytes);
        let NodeValue::StructArray {
            prefix,
            shared_header,
            elements: parsed,
        } = &tree.properties[0].value
        else {
            panic!("tagged struct array should frame");
        };
        assert_eq!(*prefix, i32::from(count).to_le_bytes());
        assert_eq!(
            *shared_header,
            struct_prop(&elements.concat())[..shared_header.len()]
        );
        assert_eq!(parsed.len(), usize::from(count));
        for (tree, original) in parsed.iter().zip(&elements) {
            assert_eq!(tree.properties.len(), 2);
            assert_eq!(tree.to_bytes().unwrap(), *original);
        }
        let summary = tree.summary();
        assert_eq!(summary.struct_arrays, 1);
        assert_eq!(summary.struct_array_elements, usize::from(count));
        assert_eq!(summary.property_count, 1 + 2 * usize::from(count));
    }

    // An ordinary Int array has no shared struct header and stays opaque.
    let extensions = [INT, &[0][..]].concat();
    let payload = [&2_i32.to_le_bytes()[..], &[5; 8]].concat();
    let bytes = stream(&[prop(ARRAY, 0, &extensions, &payload)]);
    let tree = round_trip(&bytes);
    assert!(matches!(&tree.properties[0].value, NodeValue::Opaque(kept) if *kept == payload));
}

#[test]
fn nested_arrays_inside_struct_elements_round_trip() {
    let leaf = stream(&[int(9)]);
    let inner_array = struct_array(&[leaf.clone(), leaf]);
    let element = stream(&[struct_prop(&stream(&[inner_array])), int(3)]);
    let bytes = stream(&[struct_array(&[element.clone(), element]), int(4)]);
    let tree = round_trip(&bytes);
    let NodeValue::StructArray { elements, .. } = &tree.properties[0].value else {
        panic!("outer array should frame");
    };
    let NodeValue::Struct(nested) = &elements[1].properties[0].value else {
        panic!("element struct should frame");
    };
    let NodeValue::StructArray {
        elements: inner, ..
    } = &nested.properties[0].value
    else {
        panic!("inner array should frame");
    };
    assert_eq!(inner.len(), 2);
    let summary = tree.summary();
    assert_eq!(summary.struct_arrays, 3);
    assert_eq!(summary.struct_array_elements, 6);
    assert_eq!(summary.nested_structs, 2);
    assert_eq!(summary.max_depth, 4);
    // Root array + Int; per element struct + Int, inner array, two Int leaves.
    assert_eq!(summary.property_count, 2 + 2 * (2 + 1 + 2));
    assert_eq!(summary.opaque_leaves, 1 + 2 * (1 + 2));
}

#[test]
fn depth_limit_keeps_deeper_payloads_opaque_and_round_trips() {
    let mut bytes = stream(&[int(0)]);
    for _ in 0..4 {
        bytes = stream(&[struct_prop(&bytes)]);
    }
    // Five property levels in total: four structs and the innermost Int.
    for (max_depth, framed) in [(1, 0), (2, 1), (4, 3), (5, 4), (8, 4)] {
        let tree = depth_limited(&bytes, max_depth).unwrap();
        assert_eq!(tree.to_bytes().unwrap(), bytes);
        let summary = tree.summary();
        assert_eq!(summary.nested_structs, framed);
        assert_eq!(summary.max_depth, framed + 1);
        assert_eq!(summary.opaque_leaves, 1);
    }
    let shallow = depth_limited(&bytes, 2).unwrap();
    let NodeValue::Struct(level) = &shallow.properties[0].value else {
        panic!("first level should frame");
    };
    assert!(matches!(level.properties[0].value, NodeValue::Opaque(_)));
}

#[test]
fn malformed_inputs_and_invalid_limits_fail() {
    let bytes = stream(&[int(1), struct_prop(&stream(&[int(2)]))]);
    for length in 0..bytes.len() {
        assert!(parse(&bytes[..length]).is_err(), "truncation {length}");
    }
    assert!(parse(&[bytes.as_slice(), &[0]].concat()).is_err());
    assert!(parse(&int(1)).is_err());
    assert!(parse(b"\x05\0\0\0none\0").is_err());
    for limits in [
        TreeLimits {
            max_depth: 0,
            ..TreeLimits::default()
        },
        TreeLimits {
            max_depth: 17,
            ..TreeLimits::default()
        },
        TreeLimits {
            max_nodes: 0,
            ..TreeLimits::default()
        },
        TreeLimits {
            struct_array_bytes: 0,
            ..TreeLimits::default()
        },
        TreeLimits {
            struct_array_bytes: 128 * 1024 + 1,
            ..TreeLimits::default()
        },
    ] {
        assert!(PropertyTree::parse(END, TagProfile::MemoryArchive522, limits).is_err());
    }
    // The node cap counts nested properties and fails closed.
    let at_cap = TreeLimits {
        max_nodes: 3,
        ..TreeLimits::default()
    };
    assert!(PropertyTree::parse(&bytes, TagProfile::MemoryArchive522, at_cap).is_ok());
    let below = TreeLimits {
        max_nodes: 2,
        ..TreeLimits::default()
    };
    assert_eq!(
        PropertyTree::parse(&bytes, TagProfile::MemoryArchive522, below)
            .err()
            .unwrap()
            .to_string(),
        "property tree node count exceeds selected limit"
    );
}

#[test]
fn writer_rejects_trees_inconsistent_with_declared_sizes() {
    let bytes = stream(&[int(1)]);
    let mut tree = parse(&bytes).unwrap();
    tree.properties[0].value = NodeValue::Opaque(vec![0; 5]);
    assert_eq!(
        tree.to_bytes().err().unwrap().to_string(),
        "property payload does not match declared size"
    );

    let bytes = stream(&[struct_prop(&stream(&[int(1)]))]);
    let mut tree = parse(&bytes).unwrap();
    let NodeValue::Struct(nested) = &mut tree.properties[0].value else {
        panic!("struct should frame");
    };
    nested
        .properties
        .push(parse(&stream(&[int(2)])).unwrap().properties.remove(0));
    assert!(tree.to_bytes().is_err());

    let bytes = stream(&[struct_array(&[stream(&[int(1)])])]);
    let mut tree = parse(&bytes).unwrap();
    let NodeValue::StructArray { prefix, .. } = &mut tree.properties[0].value else {
        panic!("array should frame");
    };
    *prefix = 2_i32.to_le_bytes().to_vec();
    assert_eq!(
        tree.to_bytes().err().unwrap().to_string(),
        "struct array count does not match element count"
    );
}

#[test]
fn summary_counts_kinds_without_names_or_values() {
    let bytes = stream(&[
        int(1),
        int(2),
        prop(STR, 0, &[0], SECRET),
        prop(BOOL, 0, &[0, 0], &[]),
        struct_prop(&stream(&[prop(NAME, 0, &[0], SECRET)])),
        struct_array(&[stream(&[int(3)])]),
    ]);
    let summary = round_trip(&bytes).summary();
    assert_eq!(summary.property_count, 8);
    assert_eq!(summary.properties_by_kind.int, 3);
    assert_eq!(summary.properties_by_kind.string, 1);
    assert_eq!(summary.properties_by_kind.bool, 1);
    assert_eq!(summary.properties_by_kind.name, 1);
    assert_eq!(summary.properties_by_kind.struct_, 1);
    assert_eq!(summary.properties_by_kind.array, 1);
    assert_eq!(summary.nested_structs, 1);
    assert_eq!(summary.struct_arrays, 1);
    assert_eq!(summary.struct_array_elements, 1);
    assert_eq!(summary.opaque_leaves, 6);
    assert_eq!(summary.opaque_payload_bytes, 4 * 3 + SECRET.len() * 2);
    assert_eq!(summary.max_depth, 2);
    assert!(!summary.world_imported && !summary.values_interpreted);
    let json = serde_json::to_string(&summary).unwrap();
    assert!(json.contains("\"struct\":1") && json.contains("\"enum\":0"));
    assert!(!json.contains("Sentinel") && !json.contains("Demo"));
}

#[test]
fn debug_output_contains_no_names_types_or_values() {
    let bytes = stream(&[
        prop(STR, 0, &[0], SECRET),
        struct_prop(&stream(&[prop(NAME, 0, &[0], SECRET)])),
        struct_array(&[stream(&[prop(STR, 0, &[0], SECRET)])]),
    ]);
    let tree = round_trip(&bytes);
    let debug = format!("{tree:?}");
    assert!(debug.contains("StructArray") && debug.contains("Opaque"));
    for sentinel in ["Sentinel", "Demo", "StrProperty", "NameProperty", "None"] {
        assert!(!debug.contains(sentinel), "debug leaked {sentinel}");
    }
    // Byte values are reported only as lengths, never as numeric lists.
    assert!(!debug.contains("83, 101"));
    assert!(contains(&tree.to_bytes().unwrap(), SECRET));
}

#[test]
fn base_component_wrapper_round_trips_with_trailer() {
    let fields = [
        struct_prop(&stream(&[int(1)])),
        prop(
            ENUM,
            0,
            &[b"\x02\0\0\0E\0".as_slice(), &[0]].concat(),
            SECRET,
        ),
        prop(BOOL, 0, &[1, 0], &[]),
    ];
    let bytes = [stream(&fields).as_slice(), &[0; 4]].concat();
    let component = BaseComponentTree::parse(
        &bytes,
        TagProfile::MemoryArchive522,
        bytes.len(),
        TreeLimits::default(),
    )
    .unwrap();
    assert_eq!(component.trailer, [0; 4]);
    assert_eq!(component.tree.terminator, END);
    assert!(matches!(
        component.tree.properties[0].value,
        NodeValue::Struct(_)
    ));
    assert_eq!(component.to_bytes().unwrap(), bytes);
    assert_eq!(component.summary().property_count, 4);
    assert!(!format!("{component:?}").contains("Sentinel"));

    // The reader's trailer, EOF and limit policy are inherited unchanged.
    for invalid in [
        [stream(&fields).as_slice(), &[1, 0, 0, 0]].concat(),
        [stream(&fields).as_slice(), &[0; 5]].concat(),
        stream(&fields),
    ] {
        let parsed = BaseComponentTree::parse(
            &invalid,
            TagProfile::MemoryArchive522,
            128 * 1024,
            TreeLimits::default(),
        );
        assert!(parsed.is_err());
    }
    assert!(
        BaseComponentTree::parse(
            &bytes,
            TagProfile::MemoryArchive522,
            bytes.len() - 1,
            TreeLimits::default()
        )
        .is_err()
    );
    let mut broken = component;
    broken.trailer.pop();
    assert!(broken.to_bytes().is_err());
}

const MAP: &[u8] = b"\x0c\0\0\0MapProperty\0";
const SET: &[u8] = b"\x0c\0\0\0SetProperty\0";

fn count(value: usize) -> Vec<u8> {
    i32::try_from(value).unwrap().to_le_bytes().to_vec()
}

fn string(value: &[u8]) -> Vec<u8> {
    [count(value.len() + 1).as_slice(), value, &[0]].concat()
}

/// Map header: key type, value type, absent GUID (F-556).
fn map_prop(key: &[u8], value: &[u8], payload: &[u8]) -> Vec<u8> {
    prop(MAP, 0, &[key, value, &[0]].concat(), payload)
}

fn set_prop(inner: &[u8], payload: &[u8]) -> Vec<u8> {
    prop(SET, 0, &[inner, &[0]].concat(), payload)
}

#[test]
fn map_and_set_payloads_frame_into_items_and_round_trip() {
    // Map<Str, Int>: no removals, two entries (F-559 layout).
    let scalar_map = map_prop(
        STR,
        INT,
        &[
            count(0),
            count(2),
            string(SECRET),
            vec![1, 0, 0, 0],
            string(b"second"),
            vec![2, 0, 0, 0],
        ]
        .concat(),
    );
    // Map<Int, Struct>: one removal key, one entry whose value is a tagged stream.
    let struct_map = map_prop(
        INT,
        STRUCT,
        &[
            count(1),
            vec![9, 0, 0, 0],
            count(1),
            vec![3, 0, 0, 0],
            stream(&[int(7), prop(STR, 0, &[0], &string(SECRET))]),
        ]
        .concat(),
    );
    // Set<Name> and Set<Bool>.
    let name_set = set_prop(
        NAME,
        &[count(0), count(2), string(b"A"), string(SECRET)].concat(),
    );
    let bool_set = set_prop(BOOL, &[count(0), count(2), vec![1], vec![0]].concat());
    let bytes = stream(&[scalar_map, struct_map, name_set, bool_set]);
    let tree = round_trip(&bytes);
    match &tree.properties[0].value {
        NodeValue::Map { removed, entries } => {
            assert!(removed.is_empty());
            assert_eq!(entries.len(), 2);
        }
        _ => panic!("scalar map should frame"),
    }
    match &tree.properties[1].value {
        NodeValue::Map { removed, entries } => {
            assert_eq!(removed.len(), 1);
            assert!(matches!(entries[0].1, ContainerItem::Struct(_)));
        }
        _ => panic!("struct map should frame"),
    }
    assert!(matches!(tree.properties[2].value, NodeValue::Set { .. }));
    assert!(matches!(tree.properties[3].value, NodeValue::Set { .. }));
    let summary = tree.summary();
    assert_eq!(summary.containers, 4);
    assert_eq!(summary.container_items, 4 + 3 + 2 + 2);
    assert_eq!(summary.properties_by_kind.map, 2);
    assert_eq!(summary.properties_by_kind.set, 2);
    // The struct item's two properties count as nested nodes.
    assert_eq!(summary.property_count, 6);
    let text = format!("{tree:?} {summary:?}");
    assert!(!text.contains("Sentinel"));
}

#[test]
fn malformed_or_unsupported_containers_stay_exact_opaque_leaves() {
    let cases = [
        // Trailing byte after the items.
        map_prop(STR, INT, &[count(0), count(0), vec![0]].concat()),
        // Count exceeding the remaining payload.
        set_prop(INT, &[count(0), count(5), vec![1, 0, 0, 0]].concat()),
        // Negative count.
        set_prop(
            INT,
            &count(usize::MAX >> 33)
                .iter()
                .map(|b| !b)
                .collect::<Vec<_>>(),
        ),
        // Truncated scalar item.
        set_prop(INT, &[count(0), count(1), vec![1, 0]].concat()),
        // Unsupported inner kinds: Enum and Byte items carry no enum name here.
        set_prop(ENUM, &[count(0), count(1), vec![1]].concat()),
        map_prop(
            b"\x0d\0\0\0ByteProperty\0",
            INT,
            &[count(0), count(1), vec![1, 1, 0, 0, 0]].concat(),
        ),
        // Binary struct item that is not a tagged stream.
        map_prop(
            INT,
            STRUCT,
            &[count(0), count(1), vec![1, 0, 0, 0], vec![0xff; 12]].concat(),
        ),
        // Unknown inner type name.
        set_prop(
            b"\x10\0\0\0DoublePropertyX\0",
            &[count(0), count(1), vec![0; 8]].concat(),
        ),
    ];
    for (index, case) in cases.iter().enumerate() {
        let bytes = stream(std::slice::from_ref(case));
        let tree = round_trip(&bytes);
        assert!(
            matches!(tree.properties[0].value, NodeValue::Opaque(_)),
            "case {index} should stay opaque"
        );
    }
    // An empty container needs no item width, so even an unknown inner kind
    // frames exactly.
    let empty = set_prop(
        b"\x10\0\0\0DoublePropertyX\0",
        &[count(0), count(0)].concat(),
    );
    let tree = round_trip(&stream(std::slice::from_ref(&empty)));
    assert!(matches!(tree.properties[0].value, NodeValue::Set { .. }));
}

#[test]
fn container_struct_items_respect_depth_and_node_budgets() {
    let map = map_prop(
        INT,
        STRUCT,
        &[
            count(0),
            count(1),
            vec![1, 0, 0, 0],
            stream(&[int(1), int(2)]),
        ]
        .concat(),
    );
    let bytes = stream(std::slice::from_ref(&map));
    // At depth 1 no nested stream is allowed, so the whole map stays opaque.
    let shallow = depth_limited(&bytes, 1).unwrap();
    assert!(matches!(shallow.properties[0].value, NodeValue::Opaque(_)));
    assert_eq!(shallow.to_bytes().unwrap(), bytes);
    // The map, its key and value items and the two struct properties need five
    // budget units; with one fewer the map stays an exact opaque leaf.
    let limits = TreeLimits {
        max_nodes: 4,
        ..TreeLimits::default()
    };
    let opaque = PropertyTree::parse(&bytes, TagProfile::MemoryArchive522, limits).unwrap();
    assert!(matches!(opaque.properties[0].value, NodeValue::Opaque(_)));
    assert_eq!(opaque.to_bytes().unwrap(), bytes);
    // A budget too small for a later root property still fails the parse.
    let limits = TreeLimits {
        max_nodes: 1,
        ..TreeLimits::default()
    };
    let two = stream(&[map.clone(), int(1)]);
    assert!(PropertyTree::parse(&two, TagProfile::MemoryArchive522, limits).is_err());
    let limits = TreeLimits {
        max_nodes: 5,
        ..TreeLimits::default()
    };
    let tree = PropertyTree::parse(&bytes, TagProfile::MemoryArchive522, limits).unwrap();
    assert!(matches!(tree.properties[0].value, NodeValue::Map { .. }));
}

#[test]
fn many_tiny_container_items_are_bounded_by_the_node_budget() {
    // 4,096 one-byte Bool items: within the default budget they frame, but a
    // budget of 100 stops after 99 items and keeps the payload opaque.
    let elements = vec![1u8; 4096];
    let set = set_prop(BOOL, &[count(0), count(elements.len()), elements].concat());
    let bytes = stream(std::slice::from_ref(&set));
    assert!(matches!(
        round_trip(&bytes).properties[0].value,
        NodeValue::Set { .. }
    ));
    let limits = TreeLimits {
        max_nodes: 100,
        ..TreeLimits::default()
    };
    let tree = PropertyTree::parse(&bytes, TagProfile::MemoryArchive522, limits).unwrap();
    assert!(matches!(tree.properties[0].value, NodeValue::Opaque(_)));
    assert_eq!(tree.to_bytes().unwrap(), bytes);
}

#[test]
fn a_discarded_container_attempt_does_not_consume_the_node_budget() {
    // The map frames its struct item before a trailing byte forces the opaque
    // fallback; the retained tree needs only the map and the following Int.
    let map = map_prop(
        INT,
        STRUCT,
        &[
            count(0),
            count(1),
            vec![1, 0, 0, 0],
            stream(&[int(1)]),
            vec![0],
        ]
        .concat(),
    );
    let bytes = stream(&[map, int(2)]);
    let limits = TreeLimits {
        max_nodes: 2,
        ..TreeLimits::default()
    };
    let tree = PropertyTree::parse(&bytes, TagProfile::MemoryArchive522, limits).unwrap();
    assert!(matches!(tree.properties[0].value, NodeValue::Opaque(_)));
    assert_eq!(tree.properties.len(), 2);
    assert_eq!(tree.to_bytes().unwrap(), bytes);
}
