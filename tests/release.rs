use std::path::Path;

use icarus_bevy::experimental::release::{ReleasePin, network_checksum};

#[test]
fn checksums_match_the_logged_formula() {
    // 1.0.0 is observed in both builds' logs; the others follow the same rule.
    assert_eq!(network_checksum("1.0.0"), 1_447_163_286);
    assert_eq!(
        network_checksum("3.0.30.158174-rel-DangerousHorizons"),
        1_379_612_455
    );
    assert_eq!(
        network_checksum("3.0.31.158478-rel-DangerousHorizons"),
        2_406_501_229
    );
}

#[test]
fn the_tracked_pin_is_consistent() {
    let pin = ReleasePin::load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("config/release.toml"))
        .unwrap();
    assert_eq!(pin.app_id, 2_089_300);
    assert_eq!(pin.network_checksum, network_checksum(&pin.project_version));
}

#[test]
fn a_stale_checksum_or_unknown_field_is_rejected() {
    let good = "[release]\napp_id = 2089300\nbuild_id = 1\nproject_version = \"1.0.0\"\nnetwork_checksum = 1447163286\n";
    assert!(ReleasePin::parse(good).is_ok());
    let stale = good.replace("1447163286", "1447163287");
    assert!(
        ReleasePin::parse(&stale)
            .unwrap_err()
            .to_string()
            .contains("does not match")
    );
    assert!(ReleasePin::parse(&format!("{good}extra = 1\n")).is_err());
    assert!(ReleasePin::parse(&good.replace("\"1.0.0\"", "\"\"")).is_err());
}
