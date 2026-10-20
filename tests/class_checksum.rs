//! Class network checksums reproduce values the original server exported
//! (M-707 capture; F-776, F-777) from authored engine-class schemas.
use icarus_bevy::experimental::class_checksum::{
    ChecksumProperty as P, Children, class_checksum, cpp, property_checksum, str_crc32,
};

fn vector(name: &str, struct_name: &str, offset: i32) -> P {
    P::structure(
        name,
        struct_name,
        offset,
        vec![
            P::leaf("X", "float", 0),
            P::leaf("Y", "float", 4),
            P::leaf("Z", "float", 8),
        ],
    )
}

fn rotator(name: &str, offset: i32) -> P {
    P::structure(
        name,
        "Rotator",
        offset,
        vec![
            P::leaf("Pitch", "float", 0),
            P::leaf("Yaw", "float", 4),
            P::leaf("Roll", "float", 8),
        ],
    )
}

/// `AActor`'s 10 replicated properties (public engine layout), shuffled to
/// show that the checksum sorts them itself.
fn actor() -> Vec<P> {
    let movement = P::structure(
        "ReplicatedMovement",
        "RepMovement",
        96,
        vec![
            P::leaf("RotationQuantizationLevel", "ERotatorQuantization", 51),
            vector("LinearVelocity", "Vector", 0),
            vector("AngularVelocity", "Vector", 12),
            vector("Location", "Vector", 24),
            rotator("Rotation", 36),
            P::leaf("bSimulatedPhysicSleep", "uint8", 48),
            P::leaf("bRepPhysics", "uint8", 48),
            P::leaf("LocationQuantizationLevel", "EVectorQuantization", 49),
            P::leaf("VelocityQuantizationLevel", "EVectorQuantization", 50),
        ],
    );
    let attachment = P::structure(
        "AttachmentReplication",
        "RepAttachment",
        160,
        vec![
            P::leaf("AttachParent", &cpp::object("AActor"), 0),
            vector("LocationOffset", "Vector_NetQuantize100", 8),
            vector("RelativeScale3D", "Vector_NetQuantize100", 20),
            rotator("RotationOffset", 32),
            P::leaf("AttachSocket", "FName", 44),
            P::leaf("AttachComponent", &cpp::object("USceneComponent"), 56),
        ],
    );
    vec![
        P::leaf("Instigator", &cpp::object("APawn"), 280),
        P::leaf("bTearOff", "uint8", 88),
        P::leaf("bHidden", "uint8", 88),
        P::leaf("bReplicateMovement", "uint8", 88),
        P::leaf("bCanBeDamaged", "uint8", 90),
        P::leaf("RemoteRole", &cpp::enum_as_byte("ENetRole"), 95),
        movement,
        attachment,
        P::leaf("Owner", &cpp::object("AActor"), 224),
        P::leaf("Role", &cpp::enum_as_byte("ENetRole"), 240),
    ]
}

fn player_state() -> Vec<P> {
    vec![
        P::leaf("Score", "float", 544),
        P::leaf("PlayerId", "int32", 548),
        P::leaf("Ping", "uint8", 552),
        P::leaf("bOnlySpectator", "uint8", 554),
        P::leaf("bIsSpectator", "uint8", 554),
        P::leaf("bIsInactive", "uint8", 554),
        P::leaf("bIsABot", "uint8", 554),
        P::leaf("bFromPreviousLevel", "uint8", 554),
        P::leaf("StartTime", "int32", 556),
        P::structure(
            "UniqueId",
            "UniqueNetIdRepl",
            592,
            vec![P::array(
                "ReplicationBytes",
                24,
                P::leaf("ReplicationBytes", "uint8", 0),
            )],
        ),
        P::leaf("PlayerNamePrivate", "FString", 768),
    ]
}

#[test]
fn str_crc32_hashes_each_unit_as_four_bytes() {
    // Zero-extended units: "a" is the four bytes 61 00 00 00, not one byte.
    assert_eq!(str_crc32("", 0), 0);
    assert_ne!(str_crc32("a", 0), 0xe8b7_be43); // plain CRC-32 of the single byte "a"
    assert_eq!(str_crc32("a", 0), str_crc32("a", 0));
    assert_ne!(str_crc32("ab", 0), str_crc32("ba", 0));
}

#[test]
fn actor_component_matches_the_captured_export_checksum() {
    let own = [
        P::leaf("bIsActive", "uint8", 138),
        P::leaf("bReplicates", "uint8", 136),
    ];
    assert_eq!(class_checksum(0, &own), 3_400_199_742);
}

#[test]
fn actor_and_player_state_chains_match_captured_export_checksums() {
    let actor = class_checksum(0, &actor());
    assert_eq!(actor, 2_281_024_597);
    // AInfo adds no replicated properties.
    let info = class_checksum(actor, &[]);
    assert_eq!(class_checksum(info, &player_state()), 2_779_432_370);
}

#[test]
fn rep_skip_fields_are_excluded_only_inside_structs() {
    let field = P::leaf("Hidden", "int32", 4);
    let skipped = P {
        rep_skip: true,
        ..field.clone()
    };
    let with = P::structure("S", "Thing", 0, vec![P::leaf("A", "int32", 0), skipped]);
    let without = P::structure("S", "Thing", 0, vec![P::leaf("A", "int32", 0)]);
    assert_eq!(property_checksum(&with, 7), property_checksum(&without, 7));
    let plain = P::structure("S", "Thing", 0, vec![P::leaf("A", "int32", 0), field]);
    assert_ne!(property_checksum(&plain, 7), property_checksum(&without, 7));
}

#[test]
fn equal_offsets_sort_by_case_insensitive_name() {
    let a = [P::leaf("bZeta", "uint8", 8), P::leaf("BAlpha", "uint8", 8)];
    let b = [P::leaf("BAlpha", "uint8", 8), P::leaf("bZeta", "uint8", 8)];
    assert_eq!(class_checksum(1, &a), class_checksum(1, &b));
    let mut expected = 1;
    for (name, cpp_type) in [("balpha", "uint8"), ("bzeta", "uint8")] {
        expected = str_crc32(name, expected);
        expected = str_crc32(cpp_type, expected);
        expected = str_crc32("1", expected);
    }
    assert_eq!(class_checksum(1, &a), expected);
}

#[test]
fn spelling_helpers_keep_engine_quirks() {
    assert_eq!(
        cpp::subclass_of("AGameModeBase"),
        "TSubclassOf<AGameModeBase> "
    );
    assert_eq!(cpp::delegate("OnRevived__DelegateSignature"), "FOnRevived");
    assert_eq!(cpp::enum_as_byte("ENetRole"), "TEnumAsByte<ENetRole>");
    let array = P::array("Tags", 0, P::leaf("Tags", "FName", 0));
    assert!(matches!(array.children, Children::ArrayInner(_)));
    assert_eq!(array.cpp_type, "TArray");
}
