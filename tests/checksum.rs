use icarus_bevy::experimental::{checksum_utf16, default_version_string_checksum};

// Independent oracle: Python zlib.crc32 over four little-endian bytes per UTF-16
// unit, stopping at the first zero. These strings are authored test data.
#[test]
fn checksum_matches_independently_generated_vectors() {
    for (text, seed, expected) in [
        ("", 0, 0),
        ("", 0x1234_5678, 0x1234_5678),
        ("abc", 0, 0x737f_1923),
        ("ICARUS 1.0.0", 0, 0x4acb_c48e),
        ("ÄZ😀", 0, 0x9db6_bd05),
        ("a\0b", 0, 0xa2de_4f7a),
        ("abc", 0x1234_5678, 0x221c_b1fb),
    ] {
        assert_eq!(
            checksum_utf16(&text.encode_utf16().collect::<Vec<_>>(), seed),
            expected
        );
    }
}

#[test]
fn default_transformation_only_lowercases_ascii() {
    assert_eq!(default_version_string_checksum("ICARUS 1.0.0"), 0xf323_e4bb);
    assert_eq!(default_version_string_checksum("ÄZ😀"), 0x8587_66fd);
    assert_ne!(
        default_version_string_checksum("ÄZ😀"),
        default_version_string_checksum("äz😀")
    );
    assert_eq!(default_version_string_checksum("a\0ignored"), 0xa2de_4f7a);
}

#[test]
fn finalized_seed_supports_incremental_units() {
    let units = [0xd800, 0x0061, 0xdc00, 0xffff];
    for split in 0..=units.len() {
        let prefix = checksum_utf16(&units[..split], 0);
        assert_eq!(
            checksum_utf16(&units[split..], prefix),
            checksum_utf16(&units, 0)
        );
    }
}

#[test]
fn checksum_agrees_with_observed_early_startup_value() {
    // Tool build 25621760 logged this checksum before subsequently setting
    // ProjectVersion and invalidating the cache. Not an admission value.
    assert_eq!(
        default_version_string_checksum("Icarus 1.0.0, NetCL: 0, EngineNetVer: 17, GameNetVer: 0"),
        1_447_163_286
    );
}
