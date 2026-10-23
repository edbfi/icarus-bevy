use icarus_bevy::experimental::property_tree::{
    BaseComponentTree, NodeValue, PropertyTree, TreeLimits,
};
use icarus_bevy::experimental::tagged::{TagKind, TagProfile};
use icarus_bevy::experimental::typed_value::{
    FixedStruct, Gap, Item, Scalar, TypedBudgetExceeded, TypedComponent, TypedTree, TypingBudget,
    Value,
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
    // A demoted leaf is a gap only, not also a typed string.
    assert_eq!(
        (summary.strings, summary.integers, summary.bools),
        (0, 0, 0)
    );
    assert_eq!(summary.properties, 3);
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

#[test]
fn a_container_with_an_undecodable_item_stays_one_exact_gap() {
    let set_ext = [BOOL, &[0]].concat();
    let set_payload = [&0_i32.to_le_bytes()[..], &2_i32.to_le_bytes(), &[1, 2]].concat();
    let set_kind: &[u8] = b"\x0c\0\0\0SetProperty\0";
    let bytes = stream(&[
        named("Flags", set_kind, &set_ext, &set_payload),
        named("After", INT, &[0], &7_i32.to_le_bytes()),
    ]);
    let (tree, summary) = typed_with_summary(&bytes);
    assert!(matches!(
        tree.get("Flags").unwrap().value,
        Value::Uninterpreted {
            gap: Gap::ContainerItem,
            ..
        }
    ));
    assert!(matches!(
        tree.get("After").unwrap().value,
        Value::Scalar(Scalar::Int(7))
    ));
    // Counts made while trying the container are rolled back.
    assert_eq!(
        (summary.sets, summary.container_items, summary.bools),
        (0, 0, 0)
    );
    assert_eq!(summary.uninterpreted, 1);
}

#[test]
fn fixed_struct_debug_prints_layout_not_values() {
    let printed = format!(
        "{:?} {:?}",
        FixedStruct::F32(vec![1234.5, 6789.25, 42.0]),
        FixedStruct::I64(0x5151_5151_5151)
    );
    assert_eq!(printed, "F32x3 I64");
}

fn component(properties: &[Vec<u8>]) -> BaseComponentTree {
    let bytes = [stream(properties), vec![0; 4]].concat();
    BaseComponentTree::parse(
        &bytes,
        TagProfile::MemoryArchive522,
        bytes.len(),
        TreeLimits::default(),
    )
    .unwrap()
}

fn byte_array(count: usize) -> BaseComponentTree {
    let payload = [
        i32::try_from(count).unwrap().to_le_bytes().as_slice(),
        &vec![7; count],
    ]
    .concat();
    component(&[prop(ARRAY, &[BYTE, &[0]].concat(), &payload)])
}

fn charge(component: &BaseComponentTree) -> (usize, usize) {
    let mut budget = TypingBudget::new(usize::MAX, usize::MAX);
    TypedComponent::from_component_with_budget(component, &mut budget).unwrap();
    (budget.charged_bytes(), budget.charged_elements())
}

#[test]
fn separate_valid_byte_arrays_exhaust_one_cumulative_budget() {
    let input = byte_array(256);
    let (bytes, elements) = charge(&input);
    assert_eq!(elements, 257); // One property plus the array's owned scalar slots.
    for mut budget in [
        TypingBudget::new(bytes * 3, usize::MAX),
        TypingBudget::new(usize::MAX, elements * 3),
    ] {
        // Keep the components alive together, as an import candidate does.
        let mut retained = Vec::new();
        for _ in 0..3 {
            retained.push(
                TypedComponent::from_component_with_budget(&input, &mut budget)
                    .unwrap()
                    .0,
            );
        }
        let before = (budget.charged_bytes(), budget.charged_elements());
        let error = TypedComponent::from_component_with_budget(&input, &mut budget)
            .err()
            .unwrap();
        assert!(error.downcast_ref::<TypedBudgetExceeded>().is_some());
        assert_eq!((budget.charged_bytes(), budget.charged_elements()), before);
        assert_eq!(retained.len(), 3);
    }
    // Exhausting a shared policy does not impose a limit on the old API.
    let (typed, summary) = TypedComponent::from_component(&input).unwrap();
    assert_eq!(summary.array_elements, 256);
    assert_eq!(typed.to_bytes().unwrap(), input.to_bytes().unwrap());
}

#[test]
fn typing_allowance_accepts_exact_boundaries_and_rejects_one_less() {
    let input = byte_array(17);
    let (bytes, elements) = charge(&input);
    let mut exact = TypingBudget::new(bytes, elements);
    let (typed, summary) = TypedComponent::from_component_with_budget(&input, &mut exact).unwrap();
    let (legacy, legacy_summary) = TypedComponent::from_component(&input).unwrap();
    assert_eq!(typed.to_bytes().unwrap(), legacy.to_bytes().unwrap());
    assert_eq!(summary, legacy_summary);
    assert_eq!(
        (exact.charged_bytes(), exact.charged_elements()),
        (bytes, elements)
    );
    for mut budget in [
        TypingBudget::new(bytes - 1, elements),
        TypingBudget::new(bytes, elements - 1),
    ] {
        let error = TypedComponent::from_component_with_budget(&input, &mut budget)
            .err()
            .unwrap();
        assert!(error.is::<TypedBudgetExceeded>());
        assert_eq!((budget.charged_bytes(), budget.charged_elements()), (0, 0));
    }
}

#[test]
fn empty_struct_array_elements_consume_the_shared_element_allowance() {
    let payload = [
        3_i32.to_le_bytes().as_slice(),
        &named("Empty", STRUCT, &struct_ext("Demo"), &END.repeat(3)),
    ]
    .concat();
    let input = component(&[prop(ARRAY, &[STRUCT, &[0]].concat(), &payload)]);
    let (bytes, elements) = charge(&input);
    assert_eq!(elements, 4);
    let mut budget = TypingBudget::new(bytes * 2, elements);
    let (typed, summary) = TypedComponent::from_component_with_budget(&input, &mut budget).unwrap();
    assert_eq!(summary.array_elements, 3);
    let Value::StructArray { elements, .. } = &typed.tree.properties[0].value else {
        panic!("empty struct array did not frame");
    };
    assert!(elements.iter().all(|tree| tree.properties.is_empty()));
    assert!(
        TypedComponent::from_component_with_budget(&input, &mut budget)
            .err()
            .unwrap()
            .is::<TypedBudgetExceeded>()
    );
}

#[test]
fn container_items_fixed_parts_and_decoded_strings_have_allowances() {
    let map = [
        0_i32.to_le_bytes().as_slice(),
        &1_i32.to_le_bytes(),
        &string(b"Key"),
        &string(SECRET),
    ]
    .concat();
    let set_kind = b"\x0c\0\0\0SetProperty\0";
    let set = [
        1_i32.to_le_bytes().as_slice(),
        &string(b"Removed"),
        &1_i32.to_le_bytes(),
        &string(b"Present"),
    ]
    .concat();
    let input = component(&[
        prop(MAP, &[NAME, STR, &[0]].concat(), &map),
        prop(set_kind, &[STR, &[0]].concat(), &set),
        prop(STR, &[0], &string(&[b'Q'; 512])),
        prop(STRUCT, &struct_ext("Vector"), &floats(&[1.0, 2.0, 3.0])),
    ]);
    let (bytes, elements) = charge(&input);
    assert_eq!(elements, 8); // Four properties, two Map items, two Set items.
    assert!(bytes > 512 * size_of::<u16>());
    let mut budget = TypingBudget::new(bytes, elements);
    let (typed, summary) = TypedComponent::from_component_with_budget(&input, &mut budget).unwrap();
    assert_eq!(
        (summary.maps, summary.sets, summary.fixed_structs),
        (1, 1, 1)
    );
    assert_eq!(summary.uninterpreted, 0);
    assert_eq!(typed.to_bytes().unwrap(), input.to_bytes().unwrap());
}

#[test]
fn impossible_declared_array_counts_remain_small_exact_gaps() {
    for inner in [BYTE, INT, STRUCT] {
        let input = component(&[prop(
            ARRAY,
            &[inner, &[0]].concat(),
            &[i32::MAX.to_le_bytes().as_slice(), &[7]].concat(),
        )]);
        let mut budget = TypingBudget::new(64 * 1024, 1);
        let (typed, summary) =
            TypedComponent::from_component_with_budget(&input, &mut budget).unwrap();
        assert_eq!(budget.charged_elements(), 1);
        assert_eq!(summary.uninterpreted, 1);
        assert!(matches!(
            typed.tree.properties[0].value,
            Value::Uninterpreted { .. }
        ));
        assert_eq!(typed.to_bytes().unwrap(), input.to_bytes().unwrap());
    }
}

#[test]
fn structural_typing_failure_retains_no_charge_and_budget_errors_are_private_free() {
    let good = byte_array(4);
    let (bytes, elements) = charge(&good);
    let mut budget = TypingBudget::new(bytes * 3, elements * 3);
    TypedComponent::from_component_with_budget(&good, &mut budget).unwrap();
    let before = (budget.charged_bytes(), budget.charged_elements());
    let mut invalid = byte_array(4);
    invalid.tree.properties[0].header.clear();
    let error = TypedComponent::from_component_with_budget(&invalid, &mut budget)
        .err()
        .unwrap();
    assert!(!error.is::<TypedBudgetExceeded>());
    assert_eq!((budget.charged_bytes(), budget.charged_elements()), before);
    let mut exhausted = TypingBudget::new(0, 0);
    let error = TypedComponent::from_component_with_budget(&good, &mut exhausted)
        .err()
        .unwrap();
    assert!(error.is::<TypedBudgetExceeded>());
    assert_eq!(error.to_string(), "typed allocation budget exceeded");
    assert!(!format!("{error:?}").contains("Sentinel"));
    assert_eq!(
        (exhausted.charged_bytes(), exhausted.charged_elements()),
        (0, 0)
    );
}

#[test]
fn budgeted_arrays_and_optional_gaps_preserve_legacy_typing() {
    let vectors = [
        2_i32.to_le_bytes().as_slice(),
        &named("Points", STRUCT, &struct_ext("Vector"), &floats(&[1.0; 6])),
    ]
    .concat();
    let names = [
        2_i32.to_le_bytes().as_slice(),
        &string(b"A"),
        &string(SECRET),
    ]
    .concat();
    let bad_array = [3_i32.to_le_bytes().as_slice(), &1_i32.to_le_bytes()].concat();
    let wide = [(-3_i32).to_le_bytes().as_slice(), b"A\0B\0\0\0"].concat();
    let bad_set = [
        0_i32.to_le_bytes().as_slice(),
        &2_i32.to_le_bytes(),
        &[1, 2],
    ]
    .concat();
    let set_kind = b"\x0c\0\0\0SetProperty\0";
    let input = component(&[
        prop(ARRAY, &[STRUCT, &[0]].concat(), &vectors),
        prop(ARRAY, &[NAME, &[0]].concat(), &names),
        prop(ARRAY, &[INT, &[0]].concat(), &bad_array),
        prop(STR, &[0], &wide),
        prop(set_kind, &[BOOL, &[0]].concat(), &bad_set),
    ]);
    let (bytes, elements) = charge(&input);
    let mut budget = TypingBudget::new(bytes, elements);
    let (typed, summary) = TypedComponent::from_component_with_budget(&input, &mut budget).unwrap();
    let (legacy, legacy_summary) = TypedComponent::from_component(&input).unwrap();
    assert_eq!(typed.to_bytes().unwrap(), input.to_bytes().unwrap());
    assert_eq!(typed.to_bytes().unwrap(), legacy.to_bytes().unwrap());
    assert_eq!(summary, legacy_summary);
    assert_eq!(summary.fixed_struct_arrays, 1);
    assert_eq!(summary.scalar_arrays, 1);
    assert_eq!(summary.uninterpreted, 3);
    assert_eq!(summary.gaps.get(&Gap::NonCanonical), Some(&1));
    assert_eq!(summary.gaps.get(&Gap::ContainerItem), Some(&1));
}

#[test]
fn shared_header_declared_scratch_is_checked_before_typing() {
    let payload = [
        1_i32.to_le_bytes().as_slice(),
        &named("Empty", STRUCT, &struct_ext("Demo"), END),
    ]
    .concat();
    let mut input = component(&[prop(ARRAY, &[STRUCT, &[0]].concat(), &payload)]);
    let NodeValue::StructArray { shared_header, .. } = &mut input.tree.properties[0].value else {
        panic!("authored array did not frame");
    };
    // The typed shared-header decoder uses this declared size to zero-fill its
    // scratch record. Alter only that size in a public, caller-owned tree.
    let size_offset = string(b"Empty").len() + STRUCT.len();
    shared_header[size_offset..size_offset + 4].copy_from_slice(&i32::MAX.to_le_bytes());
    let mut budget = TypingBudget::new(64 * 1024, 100);
    let error = TypedComponent::from_component_with_budget(&input, &mut budget)
        .err()
        .unwrap();
    assert!(error.is::<TypedBudgetExceeded>());
    assert_eq!((budget.charged_bytes(), budget.charged_elements()), (0, 0));
}
