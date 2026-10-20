use icarus_bevy::experimental::property_tree::{BaseComponentTree, PropertyTree, TreeLimits};
use icarus_bevy::experimental::tagged::{TagKind, TagProfile};
use icarus_bevy::experimental::typed_value::{
    FixedStruct, Gap, Item, Scalar, TypedComponent, TypedTree, Value,
};

const END: &[u8] = b"\x05\0\0\0None\0";
const ARRAY: &[u8] = b"\x0e\0\0\0ArrayProperty\0";
const STRUCT: &[u8] = b"\x0f\0\0\0StructProperty\0";
const STR: &[u8] = b"\x0c\0\0\0StrProperty\0";
const INT: &[u8] = b"\x0c\0\0\0IntProperty\0";
const INT64: &[u8] = b"\x0e\0\0\0Int64Property\0";
const UINT32: &[u8] = b"\x0f\0\0\0UInt32Property\0";
const NAME: &[u8] = b"\x0d\0\0\0NameProperty\0";
const FLOAT: &[u8] = b"\x0e\0\0\0FloatProperty\0";
const BOOL: &[u8] = b"\x0d\0\0\0BoolProperty\0";
const BYTE: &[u8] = b"\x0d\0\0\0ByteProperty\0";
const ENUM: &[u8] = b"\x0d\0\0\0EnumProperty\0";
const MAP: &[u8] = b"\x0c\0\0\0MapProperty\0";
const NONE_NAME: &[u8] = b"\x05\0\0\0None\0";
const SECRET: &[u8] = b"SentinelValueQQ";

// Independently authored headers; property names are chosen per test.
fn named(name: &str, kind: &[u8], extensions: &[u8], payload: &[u8]) -> Vec<u8> {
    [
        string(name.as_bytes()).as_slice(),
        kind,
        &i32::try_from(payload.len()).unwrap().to_le_bytes(),
        &0_i32.to_le_bytes(),
        extensions,
        payload,
    ]
    .concat()
}

fn prop(kind: &[u8], extensions: &[u8], payload: &[u8]) -> Vec<u8> {
    named("SentinelNameQQ", kind, extensions, payload)
}

fn string(value: &[u8]) -> Vec<u8> {
    let count = i32::try_from(value.len() + 1).unwrap().to_le_bytes();
    [&count[..], value, &[0]].concat()
}

fn struct_ext(type_name: &str) -> Vec<u8> {
    [string(type_name.as_bytes()).as_slice(), &[7; 16], &[0]].concat()
}

fn stream(properties: &[Vec<u8>]) -> Vec<u8> {
    [properties.concat().as_slice(), END].concat()
}

fn floats(values: &[f32]) -> Vec<u8> {
    values.iter().flat_map(|v| v.to_le_bytes()).collect()
}

fn typed(bytes: &[u8]) -> TypedTree {
    let tree =
        PropertyTree::parse(bytes, TagProfile::MemoryArchive522, TreeLimits::default()).unwrap();
    let (typed, _) = TypedTree::from_tree(&tree).unwrap();
    assert_eq!(
        typed.to_bytes().unwrap(),
        bytes,
        "typed tree must re-encode exactly"
    );
    typed
}

fn typed_with_summary(
    bytes: &[u8],
) -> (
    TypedTree,
    icarus_bevy::experimental::typed_value::TypedSummary,
) {
    let tree =
        PropertyTree::parse(bytes, TagProfile::MemoryArchive522, TreeLimits::default()).unwrap();
    let (typed, summary) = TypedTree::from_tree(&tree).unwrap();
    assert_eq!(typed.to_bytes().unwrap(), bytes);
    (typed, summary)
}

#[test]
fn scalars_type_and_re_encode_exactly() {
    let bytes = stream(&[
        named("A", INT, &[0], &(-7_i32).to_le_bytes()),
        named("B", UINT32, &[0], &0xdead_beef_u32.to_le_bytes()),
        named("C", INT64, &[0], &(-9_000_000_000_i64).to_le_bytes()),
        named("D", FLOAT, &[0], &1.5_f32.to_le_bytes()),
        named("E", BOOL, &[1, 0], &[]),
        named("F", BOOL, &[0, 0], &[]),
        named("G", BYTE, &[NONE_NAME, &[0]].concat(), &[200]),
        named(
            "H",
            BYTE,
            &[string(b"EDemo").as_slice(), &[0]].concat(),
            &string(b"EDemo::B"),
        ),
        named(
            "I",
            ENUM,
            &[string(b"EDemo").as_slice(), &[0]].concat(),
            &string(b"EDemo::C"),
        ),
        named("J", STR, &[0], &string(SECRET)),
        named("K", NAME, &[0], &string(b"Row_1")),
    ]);
    let (tree, summary) = typed_with_summary(&bytes);
    let scalar = |name: &str| match &tree.get(name).unwrap().value {
        Value::Scalar(scalar) => scalar.clone(),
        _ => panic!("{name} is not a scalar"),
    };
    assert_eq!(scalar("A"), Scalar::Int(-7));
    assert_eq!(scalar("B"), Scalar::UInt32(0xdead_beef));
    assert_eq!(scalar("C"), Scalar::Int64(-9_000_000_000));
    assert_eq!(scalar("D"), Scalar::Float(1.5));
    assert_eq!(scalar("E"), Scalar::Bool(true));
    assert_eq!(scalar("F"), Scalar::Bool(false));
    assert_eq!(scalar("G"), Scalar::Byte(200));
    assert_eq!(
        scalar("H"),
        Scalar::Enum("EDemo::B".encode_utf16().collect())
    );
    assert_eq!(
        scalar("I"),
        Scalar::Enum("EDemo::C".encode_utf16().collect())
    );
    assert_eq!(
        scalar("J"),
        Scalar::Str(SECRET.iter().map(|b| u16::from(*b)).collect())
    );
    assert_eq!(scalar("K"), Scalar::Name("Row_1".encode_utf16().collect()));
    assert_eq!(summary.properties, 11);
    assert_eq!(summary.uninterpreted, 0);
    assert_eq!((summary.integers, summary.bools, summary.enums), (3, 2, 2));
}

#[test]
fn float_bits_including_nan_payloads_survive() {
    let nan = f32::from_bits(0x7fc0_1234);
    let bytes = stream(&[named("N", FLOAT, &[0], &nan.to_bits().to_le_bytes())]);
    let tree = typed(&bytes);
    let Value::Scalar(Scalar::Float(value)) = tree.get("N").unwrap().value else {
        panic!("not a float");
    };
    assert_eq!(value.to_bits(), 0x7fc0_1234);
}

#[test]
fn fixed_and_tagged_structs_type_with_their_names() {
    let translation = floats(&[1.0, -2.0, 3.5]);
    let rotation = floats(&[0.0, 0.0, 0.707_106_77, 0.707_106_77]);
    let transform = stream(&[
        named("Rotation", STRUCT, &struct_ext("Quat"), &rotation),
        named("Translation", STRUCT, &struct_ext("Vector"), &translation),
        named(
            "Scale3D",
            STRUCT,
            &struct_ext("Vector"),
            &floats(&[1.0, 1.0, 1.0]),
        ),
    ]);
    let bytes = stream(&[
        named("Where", STRUCT, &struct_ext("Transform"), &transform),
        named("Id", STRUCT, &struct_ext("Guid"), &[3; 16]),
        named(
            "Cell",
            STRUCT,
            &struct_ext("IntPoint"),
            &[[5, 0, 0, 0], [0xff; 4]].concat(),
        ),
    ]);
    let (tree, summary) = typed_with_summary(&bytes);
    let Value::TaggedStruct { tree: inner, .. } = &tree.get("Where").unwrap().value else {
        panic!("transform did not frame as tags");
    };
    let Value::FixedStruct { value, .. } = &inner.get("Translation").unwrap().value else {
        panic!("translation is not fixed");
    };
    assert_eq!(value.floats::<3>(), Some([1.0, -2.0, 3.5]));
    let Value::FixedStruct { value, .. } = &inner.get("Rotation").unwrap().value else {
        panic!("rotation is not fixed");
    };
    assert_eq!(value.floats::<4>().unwrap()[2], 0.707_106_77);
    let Value::FixedStruct { value, .. } = &tree.get("Id").unwrap().value else {
        panic!("guid is not fixed");
    };
    assert_eq!(*value, FixedStruct::U32(vec![0x0303_0303; 4]));
    let Value::FixedStruct { value, .. } = &tree.get("Cell").unwrap().value else {
        panic!("int point is not fixed");
    };
    assert_eq!(*value, FixedStruct::I32(vec![5, -1]));
    assert_eq!(summary.fixed_structs, 5);
    assert_eq!(summary.tagged_structs, 1);
    assert_eq!(summary.uninterpreted, 0);
}

#[test]
fn unknown_or_misfit_structs_stay_exact_gaps() {
    let bytes = stream(&[
        named("U", STRUCT, &struct_ext("MysteryBlob"), &[1, 2, 3, 4, 5]),
        named("W", STRUCT, &struct_ext("Vector"), &[0; 8]),
    ]);
    let (tree, summary) = typed_with_summary(&bytes);
    assert!(matches!(
        tree.get("U").unwrap().value,
        Value::Uninterpreted {
            gap: Gap::UnknownStruct,
            ..
        }
    ));
    assert!(matches!(
        tree.get("W").unwrap().value,
        Value::Uninterpreted {
            gap: Gap::FixedStructWidth,
            ..
        }
    ));
    assert_eq!(summary.uninterpreted, 2);
    assert_eq!(summary.uninterpreted_bytes, 13);
    assert_eq!(summary.gaps.get(&Gap::UnknownStruct), Some(&1));
}

#[test]
fn scalar_and_fixed_struct_arrays_type_by_inner_kind() {
    let ints = [
        &3_i32.to_le_bytes()[..],
        &1_i32.to_le_bytes(),
        &2_i32.to_le_bytes(),
        &(-3_i32).to_le_bytes(),
    ]
    .concat();
    let names = [&2_i32.to_le_bytes()[..], &string(b"A"), &string(b"Bb")].concat();
    let bools = [&2_i32.to_le_bytes()[..], &[1, 0]].concat();
    let vectors_body = floats(&[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    let vectors = [
        &2_i32.to_le_bytes()[..],
        &named("Points", STRUCT, &struct_ext("Vector"), &vectors_body),
    ]
    .concat();
    let array_ext = |inner: &[u8]| [inner, &[0]].concat();
    let bytes = stream(&[
        named("Ints", ARRAY, &array_ext(INT), &ints),
        named("Names", ARRAY, &array_ext(NAME), &names),
        named("Flags", ARRAY, &array_ext(BOOL), &bools),
        named("Points", ARRAY, &array_ext(STRUCT), &vectors),
    ]);
    let (tree, summary) = typed_with_summary(&bytes);
    let Value::ScalarArray { inner, items } = &tree.get("Ints").unwrap().value else {
        panic!("ints not typed");
    };
    assert_eq!(*inner, TagKind::Int);
    assert_eq!(items, &[Scalar::Int(1), Scalar::Int(2), Scalar::Int(-3)]);
    let Value::ScalarArray { items, .. } = &tree.get("Names").unwrap().value else {
        panic!("names not typed");
    };
    assert_eq!(items.len(), 2);
    let Value::ScalarArray { items, .. } = &tree.get("Flags").unwrap().value else {
        panic!("bools not typed");
    };
    assert_eq!(items, &[Scalar::Bool(true), Scalar::Bool(false)]);
    let Value::FixedStructArray { elements, .. } = &tree.get("Points").unwrap().value else {
        panic!("vectors not typed");
    };
    assert_eq!(elements[1].floats::<3>(), Some([4.0, 5.0, 6.0]));
    assert_eq!(summary.scalar_arrays, 3);
    assert_eq!(summary.fixed_struct_arrays, 1);
    assert_eq!(summary.array_elements, 9);
    assert_eq!(summary.uninterpreted, 0);
}

#[test]
fn malformed_arrays_stay_exact_gaps() {
    let array_ext = |inner: &[u8]| [inner, &[0]].concat();
    let short = [&3_i32.to_le_bytes()[..], &1_i32.to_le_bytes()].concat();
    let trailing = [&1_i32.to_le_bytes()[..], &1_i32.to_le_bytes(), &[9]].concat();
    let bad_bool = [&1_i32.to_le_bytes()[..], &[2]].concat();
    let negative = (-1_i32).to_le_bytes().to_vec();
    let text_inner = b"\x0d\0\0\0TextProperty\0";
    let bytes = stream(&[
        named("Short", ARRAY, &array_ext(INT), &short),
        named("Trailing", ARRAY, &array_ext(INT), &trailing),
        named("BadBool", ARRAY, &array_ext(BOOL), &bad_bool),
        named("Negative", ARRAY, &array_ext(INT), &negative),
        named("Texts", ARRAY, &array_ext(text_inner), &0_i32.to_le_bytes()),
    ]);
    let (tree, summary) = typed_with_summary(&bytes);
    for name in ["Short", "Trailing", "BadBool", "Negative"] {
        assert!(
            matches!(
                tree.get(name).unwrap().value,
                Value::Uninterpreted {
                    gap: Gap::ArrayFraming,
                    ..
                }
            ),
            "{name}"
        );
    }
    assert!(matches!(
        tree.get("Texts").unwrap().value,
        Value::Uninterpreted {
            gap: Gap::UnsupportedArrayInner,
            ..
        }
    ));
    assert_eq!(summary.uninterpreted, 5);
}

#[test]
fn noncanonical_strings_and_bools_are_demoted_not_rewritten() {
    // A wide-encoded pure-ASCII string decodes but would re-encode narrow.
    let wide = [&(-3_i32).to_le_bytes()[..], b"A\0B\0\0\0"].concat();
    let bytes = stream(&[
        named("Wide", STR, &[0], &wide),
        named("BoolPayload", BOOL, &[1, 0], &[1]),
        named("ShortInt", INT, &[0], &[1, 2]),
    ]);
    let (tree, summary) = typed_with_summary(&bytes);
    assert!(matches!(
        tree.get("Wide").unwrap().value,
        Value::Uninterpreted {
            gap: Gap::NonCanonical,
            ..
        }
    ));
    assert!(matches!(
        tree.get("BoolPayload").unwrap().value,
        Value::Uninterpreted {
            gap: Gap::ScalarFraming,
            ..
        }
    ));
    assert!(matches!(
        tree.get("ShortInt").unwrap().value,
        Value::Uninterpreted {
            gap: Gap::ScalarFraming,
            ..
        }
    ));
    assert_eq!(summary.uninterpreted, 3);
}

#[test]
fn map_items_and_struct_arrays_type_recursively() {
    let entry = stream(&[named("Count", INT, &[0], &4_i32.to_le_bytes())]);
    let map_payload = [
        &0_i32.to_le_bytes()[..],
        &1_i32.to_le_bytes(),
        &string(b"Key"),
        &entry,
    ]
    .concat();
    let map_ext = [NAME, STRUCT, &[0]].concat();
    let element = stream(&[named("Hp", FLOAT, &[0], &2.0_f32.to_le_bytes())]);
    let struct_array = [
        &2_i32.to_le_bytes()[..],
        &named(
            "Items",
            STRUCT,
            &struct_ext("Demo"),
            &[element.clone(), element].concat(),
        ),
    ]
    .concat();
    let bytes = stream(&[
        named("Table", MAP, &map_ext, &map_payload),
        named("Items", ARRAY, &[STRUCT, &[0]].concat(), &struct_array),
    ]);
    let (tree, summary) = typed_with_summary(&bytes);
    let Value::Map { removed, entries } = &tree.get("Table").unwrap().value else {
        panic!("map not typed");
    };
    assert!(removed.is_empty());
    let (Item::Scalar(Scalar::Name(key)), Item::Struct(value)) = &entries[0] else {
        panic!("map entry not typed");
    };
    assert_eq!(*key, "Key".encode_utf16().collect::<Vec<_>>());
    assert!(matches!(
        value.get("Count").unwrap().value,
        Value::Scalar(Scalar::Int(4))
    ));
    let Value::StructArray {
        type_name,
        elements,
        ..
    } = &tree.get("Items").unwrap().value
    else {
        panic!("struct array not typed");
    };
    assert_eq!(*type_name, "Demo".encode_utf16().collect::<Vec<_>>());
    assert_eq!(elements.len(), 2);
    assert_eq!((summary.maps, summary.struct_arrays), (1, 1));
    assert_eq!(summary.uninterpreted, 0);
}

#[test]
fn base_component_trailer_round_trips() {
    let body = [
        stream(&[named("V", INT, &[0], &1_i32.to_le_bytes())]),
        vec![0; 4],
    ]
    .concat();
    let component = BaseComponentTree::parse(
        &body,
        TagProfile::MemoryArchive522,
        1024,
        TreeLimits::default(),
    )
    .unwrap();
    let (typed, summary) = TypedComponent::from_component(&component).unwrap();
    assert_eq!(typed.to_bytes().unwrap(), body);
    assert_eq!(summary.properties, 1);
}

#[test]
fn debug_output_never_contains_names_or_values() {
    let bytes = stream(&[
        prop(STR, &[0], &string(SECRET)),
        prop(INT, &[0], &0x5151_5151_i32.to_le_bytes()),
    ]);
    let tree = typed(&bytes);
    let debug = format!("{tree:?} {:?}", tree.properties);
    assert!(!debug.contains("Sentinel"));
    assert!(!debug.contains("1364283729"));
    for property in &tree.properties {
        if let Value::Scalar(scalar) = &property.value {
            let printed = format!("{scalar:?}");
            assert!(!printed.contains("Sentinel") && !printed.contains("1364283729"));
        }
    }
}
