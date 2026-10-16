use icarus_bevy::experimental::packet::{BitReader, BitWriter};
use icarus_bevy::experimental::rep_layout::{TagEncoding, read_tag_net_index, write_tag_net_index};
use icarus_bevy::experimental::tag_dictionary::{
    MAX_TAG_NAME_BYTES, MAX_TAGS, TagDictionary, TagDictionaryError, TagIndexKind,
};

// Independently authored names and bit fixtures; no original dictionary input.
const NAMES: [&str; 5] = [
    "Authored.Zulu",
    "Authored.Alpha",
    "Authored.Middle",
    "Authored.MixedCase",
    "Authored.Final",
];

fn dictionary() -> TagDictionary {
    TagDictionary::from_ordered_names(NAMES, 4, 2, 3).unwrap()
}

#[test]
fn supplied_order_and_spelling_are_preserved_without_inserting_parents() {
    let table = dictionary();
    assert_eq!(table.len(), NAMES.len());
    assert!(!table.is_empty());
    for (index, name) in NAMES.iter().enumerate() {
        assert_eq!(table.index_of(name), Some(index as u16));
        assert_eq!(table.name_of(index as u16), Some(*name));
        assert_eq!(table.classify(index as u16), TagIndexKind::Assigned);
    }
    assert_eq!(table.index_of("AUTHORED.MIXEDCASE"), Some(3));
    assert_eq!(table.index_of("Authored"), None);
    assert_eq!(table.index_of("Authored.Unknown"), None);
    assert_eq!(table.index_of("NonAscii.\u{e9}"), None);
    assert_eq!(table.container_bits(), 3);
    assert_eq!(
        table.encoding(),
        TagEncoding::NetIndex {
            first_segment: 2,
            total_bits: 4,
        }
    );
}

#[test]
fn printable_ascii_names_preserve_punctuation_without_normalization() {
    let names = ["Fixture.Cost$", "Fixture.With space", "Fixture.Punct-+:()"];
    let table = TagDictionary::from_ordered_names(names, 3, 2, 8).unwrap();
    for (index, name) in names.iter().enumerate() {
        assert_eq!(table.name_of(index as u16), Some(*name));
        assert_eq!(table.index_of(name), Some(index as u16));
    }
    assert_eq!(table.index_of("FIXTURE.COST$"), Some(0));
    assert_eq!(table.index_of("Fixture.Cost"), None);
    for name in ["Fixture.Tab\t", "Fixture.Nul\0", "Fixture.Del\u{7f}"] {
        assert_eq!(
            TagDictionary::from_ordered_names([name], 2, 1, 1).unwrap_err(),
            TagDictionaryError::InvalidName
        );
    }
}

#[test]
fn reserved_slots_are_distinct_and_never_resolve_to_names() {
    let table = dictionary();
    assert_eq!(table.unused_index(), 5);
    assert_eq!(table.invalid_index(), 6);
    assert_eq!(table.classify(5), TagIndexKind::Unused);
    assert_eq!(table.classify(6), TagIndexKind::Invalid);
    for index in [7, 15, 16, u16::MAX] {
        assert_eq!(table.classify(index), TagIndexKind::OutOfRange);
    }
    for index in [5, 6, 7, u16::MAX] {
        assert_eq!(table.name_of(index), None);
    }
    let empty = TagDictionary::from_ordered_names(Vec::<&str>::new(), 1, 1, 1).unwrap();
    assert!(empty.is_empty());
    assert_eq!(empty.classify(0), TagIndexKind::Unused);
    assert_eq!(empty.classify(1), TagIndexKind::Invalid);
    assert_eq!(empty.classify(2), TagIndexKind::OutOfRange);
}

#[test]
fn split_and_unsplit_fixtures_round_trip_through_the_existing_codec() {
    // The split form writes 2 low bits and a continuation bit; a set
    // continuation adds the remaining 2 high bits, all least-significant first.
    let table = dictionary();
    for (index, bytes, bit_len) in [
        (0, vec![0x00], 3),
        (3, vec![0x03], 3),
        (4, vec![0x0c], 5),
        (5, vec![0x0d], 5),
        (6, vec![0x0e], 5),
        (15, vec![0x1f], 5),
    ] {
        let TagEncoding::NetIndex {
            first_segment,
            total_bits,
        } = table.encoding()
        else {
            unreachable!();
        };
        let mut reader = BitReader::new(&bytes, bit_len).unwrap();
        let decoded = read_tag_net_index(&mut reader, first_segment, total_bits).unwrap();
        assert_eq!(decoded, index);
        assert_eq!(reader.remaining(), 0);
        if table.classify(decoded) == TagIndexKind::Assigned {
            assert_eq!(table.index_of(table.name_of(decoded).unwrap()), Some(index));
        }
        let mut writer = BitWriter::new();
        write_tag_net_index(&mut writer, decoded, first_segment, total_bits).unwrap();
        assert_eq!(writer.bit_len(), bit_len);
        assert_eq!(writer.as_bytes(), bytes);
    }
    let unsplit = TagDictionary::from_ordered_names(NAMES, 3, 3, 1).unwrap();
    let TagEncoding::NetIndex {
        first_segment,
        total_bits,
    } = unsplit.encoding()
    else {
        unreachable!();
    };
    let bytes = [4];
    let mut reader = BitReader::new(&bytes, 3).unwrap();
    let index = read_tag_net_index(&mut reader, first_segment, total_bits).unwrap();
    assert_eq!(unsplit.name_of(index), Some(NAMES[4]));
    let mut writer = BitWriter::new();
    write_tag_net_index(&mut writer, index, first_segment, total_bits).unwrap();
    assert_eq!(writer.bit_len(), 3);
    assert_eq!(writer.as_bytes(), bytes);
    assert!(write_tag_net_index(&mut writer, 8, first_segment, total_bits).is_err());
}

#[test]
fn names_and_duplicates_fail_without_exposing_supplied_text() {
    for names in [
        ["Authored.Duplicate", "Authored.Duplicate"],
        ["Authored.Case", "authored.CASE"],
    ] {
        assert_eq!(
            TagDictionary::from_ordered_names(names, 3, 2, 1).unwrap_err(),
            TagDictionaryError::DuplicateName
        );
    }
    for name in ["", ".Tag", "Tag.", "Tag..Child", "Tag\nChild", "Tag.\u{e9}"] {
        assert_eq!(
            TagDictionary::from_ordered_names([name], 2, 1, 1).unwrap_err(),
            TagDictionaryError::InvalidName
        );
    }
    let longest = "x".repeat(MAX_TAG_NAME_BYTES);
    assert!(TagDictionary::from_ordered_names([&longest], 2, 1, 1).is_ok());
    let too_long = "x".repeat(MAX_TAG_NAME_BYTES + 1);
    assert_eq!(
        TagDictionary::from_ordered_names([&too_long], 2, 1, 1).unwrap_err(),
        TagDictionaryError::InvalidName
    );
    let table = dictionary();
    let debug = format!("{table:?}");
    assert!(NAMES.iter().all(|name| !debug.contains(name)));
    let error =
        TagDictionary::from_ordered_names(["Authored.Private", "AUTHORED.PRIVATE"], 3, 2, 1)
            .unwrap_err();
    assert!(!format!("{error:?} {error}").contains("Private"));
}

#[test]
fn zero_first_segment_uses_the_supported_unsplit_codec() {
    let table = TagDictionary::from_ordered_names(NAMES, 3, 0, 1).unwrap();
    let TagEncoding::NetIndex {
        first_segment,
        total_bits,
    } = table.encoding()
    else {
        unreachable!();
    };
    let mut writer = BitWriter::new();
    write_tag_net_index(&mut writer, 4, first_segment, total_bits).unwrap();
    assert_eq!(writer.bit_len(), 3);
    assert_eq!(writer.as_bytes(), [4]);
    let mut reader = BitReader::new(writer.as_bytes(), writer.bit_len()).unwrap();
    let index = read_tag_net_index(&mut reader, first_segment, total_bits).unwrap();
    assert_eq!(table.name_of(index), Some(NAMES[4]));
    assert_eq!(reader.remaining(), 0);
}

#[test]
fn explicit_widths_and_count_bound_include_both_reserved_slots() {
    assert_eq!(
        TagDictionary::from_ordered_names(NAMES, 3, 2, 8)
            .unwrap()
            .container_bits(),
        8
    );
    for (total, first, container, expected) in [
        (0, 1, 1, TagDictionaryError::InvalidTotalBits),
        (17, 1, 1, TagDictionaryError::InvalidTotalBits),
        (3, 17, 1, TagDictionaryError::InvalidFirstSegment),
        (3, 1, 0, TagDictionaryError::InvalidContainerBits),
        (3, 1, 9, TagDictionaryError::InvalidContainerBits),
        (2, 1, 1, TagDictionaryError::InsufficientIndexWidth),
    ] {
        assert_eq!(
            TagDictionary::from_ordered_names(NAMES, total, first, container).unwrap_err(),
            expected
        );
    }
    let full = TagDictionary::from_ordered_names(
        (0..MAX_TAGS).map(|index| format!("Authored.T{index}")),
        16,
        16,
        8,
    )
    .unwrap();
    assert_eq!(full.len(), MAX_TAGS);
    assert_eq!(full.invalid_index(), u16::MAX);
    assert_eq!(full.classify(u16::MAX - 2), TagIndexKind::Assigned);
    assert_eq!(full.classify(u16::MAX - 1), TagIndexKind::Unused);
    assert_eq!(full.classify(u16::MAX), TagIndexKind::Invalid);
    assert_eq!(
        TagDictionary::from_ordered_names(
            (0..=MAX_TAGS).map(|index| format!("Authored.T{index}")),
            16,
            16,
            8,
        )
        .unwrap_err(),
        TagDictionaryError::TooManyTags
    );
}
