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
    // Independent values: standard reflected CRC-32 (0xedb88320, zlib) over
    // `61 00 00 00` and over `4e 00 00 00 6f 00 00 00 6e 00 00 00 65 00 00 00`.
    assert_eq!(str_crc32("a", 0), 2_732_478_330);
    assert_eq!(str_crc32("None", 0), 1_897_401_797);
    assert_ne!(str_crc32("a", 0), 0xe8b7_be43); // plain CRC-32 of the single byte "a"
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

fn meta_resource(name: &str, offset: i32) -> P {
    P::structure(
        name,
        "MetaResource",
        offset,
        vec![
            P::leaf("MetaRow", "FString", 0),
            P::leaf("Count", "int32", 16),
        ],
    )
}

fn type_value(name: &str, struct_name: &str) -> P {
    P::structure(
        name,
        struct_name,
        0,
        vec![P::leaf("Type", "int32", 0), P::leaf("Value", "int32", 4)],
    )
}

fn meta_item(name: &str, offset: i32) -> P {
    P::structure(
        name,
        "MetaItem",
        offset,
        vec![
            P::leaf("ItemStaticRow", "FString", 0),
            P::array(
                "Properties",
                16,
                type_value("Properties", "DynamicProperty"),
            ),
            P::array("Stats", 32, type_value("Stats", "Stat")),
            P::leaf("ID", "FString", 48),
        ],
    )
}

fn dropship(name: &str, offset: i32) -> P {
    P::structure(
        name,
        "Dropship",
        offset,
        vec![
            P::leaf("Name", "FString", 0),
            P::leaf("Type", "EDropshipType", 16),
            P::leaf("DropshipID", "int32", 20),
            P::leaf("InUse", "bool", 24),
            meta_item("TOP_Part", 32),
            meta_item("MID_Part", 96),
            meta_item("BTM_Part", 160),
        ],
    )
}

fn prospect_info(name: &str) -> P {
    let member = P::structure(
        "AssociatedMembers",
        "AssociatedMemberInfo",
        0,
        vec![
            P::leaf("AccountName", "FString", 0),
            P::leaf("CharacterName", "FString", 16),
            P::leaf("UserID", "FString", 32),
            P::leaf("ChrSlot", "int32", 48),
            P::leaf("Experience", "int32", 52),
            P::leaf("Status", "EProspectLocation", 56),
            P::leaf("Settled", "bool", 57),
            P::leaf("IsCurrentlyPlaying", "bool", 58),
        ],
    );
    let setting = P::structure(
        "CustomSettings",
        "CustomGameSetting",
        0,
        vec![
            P::leaf("SettingRowName", "FString", 0),
            P::leaf("SettingValue", "int32", 16),
        ],
    );
    P::structure(
        name,
        "ProspectInfo",
        0,
        vec![
            P::leaf("ProspectID", "FString", 0),
            P::leaf("ClaimedAccountID", "FString", 16),
            P::leaf("ClaimedAccountCharacter", "int32", 32),
            P::leaf("ProspectDTKey", "FString", 40),
            P::leaf("FactionMissionDTKey", "FString", 56),
            P::leaf("LobbyName", "FString", 72),
            P::leaf("ExpireTime", "int64", 88),
            P::leaf("ProspectState", "EProspectState", 96),
            P::array("AssociatedMembers", 104, member),
            P::leaf("Cost", "int32", 120),
            P::leaf("Reward", "int32", 124),
            P::leaf("Difficulty", "EMissionDifficulty", 128),
            P::leaf("Insurance", "bool", 129),
            P::leaf("NoRespawns", "bool", 130),
            P::leaf("ElapsedTime", "int32", 132),
            P::leaf("SelectedDropPoint", "int32", 136),
            P::array("CustomSettings", 144, setting),
        ],
    )
}

fn notification(name: &str) -> P {
    let attachment = P::structure(
        "Attachments",
        "Attachment",
        72,
        vec![
            P::array("Items", 0, meta_item("Items", 0)),
            P::array("Exotics", 16, meta_resource("Exotics", 0)),
            P::leaf("Credits", "int32", 32),
        ],
    );
    P::structure(
        name,
        "Notification",
        0,
        vec![
            P::leaf("NotificationUID", "FString", 0),
            P::leaf("Type", "ENotificationType", 16),
            P::leaf("Title", "FString", 24),
            P::leaf("Message", "FString", 40),
            P::leaf("ProspectID", "FString", 56),
            attachment,
            P::leaf("Read", "bool", 112),
            P::leaf("SentToPlayer", "bool", 113),
        ],
    )
}

/// `UBackendProxyComponent`'s own replicated properties, read from the pinned
/// 3.0.32 server's reflection tables (M-1091).
fn backend_proxy_component() -> Vec<P> {
    let loadout = P::structure(
        "CharacterLoadout",
        "CharacterLoadout",
        848,
        vec![
            meta_item("EnviroSuit", 0),
            dropship("Dropship", 64),
            P::array("MetaItems", 288, meta_item("MetaItems", 0)),
            P::leaf("Valid", "bool", 304),
        ],
    );
    vec![
        P::array("MetaResources", 704, meta_resource("MetaResources", 0)),
        P::leaf("Credits", "int32", 720),
        P::array("MetaInventory", 728, meta_item("MetaInventory", 0)),
        P::array("LoadoutInventory", 744, meta_item("LoadoutInventory", 0)),
        P::array("Dropships", 760, dropship("Dropships", 0)),
        P::leaf("PreparedDropship", "int32", 776),
        meta_item("PreparedEnvirosuit", 784),
        loadout,
        P::leaf("PreparedLoadout", "bool", 1160),
        P::array("Prospects", 1168, prospect_info("Prospects")),
        P::array("Notifications", 1184, notification("Notifications")),
    ]
}

#[test]
fn backend_proxy_component_matches_the_controller_subobject_checksum() {
    // The controller's `BackendProxyComponent` subobject exports this checksum
    // (`ReleasePlayerControllerProfile::icarus_survival`, F-779/F-780), so the
    // subobject is a plain `UBackendProxyComponent`, whose net cache indexes its
    // profile RPCs (F-954).
    let actor_component = class_checksum(
        0,
        &[
            P::leaf("bIsActive", "uint8", 138),
            P::leaf("bReplicates", "uint8", 136),
        ],
    );
    assert_eq!(
        class_checksum(actor_component, &backend_proxy_component()),
        4_100_406_609
    );
}

/// A fast-array item: `FFastArraySerializerItem`'s three fields are
/// `CPF_RepSkip`, then a row handle (`FRowHandle` fields) and the item's own.
fn fast_array_item(struct_name: &str, row: (&str, &str), fields: Vec<P>) -> P {
    let skipped = |name: &str, offset| P {
        rep_skip: true,
        ..P::leaf(name, "int32", offset)
    };
    let mut all = vec![
        skipped("ReplicationID", 0),
        skipped("ReplicationKey", 4),
        skipped("MostRecentArrayReplicationKey", 8),
        P::structure(
            row.0,
            row.1,
            12,
            vec![
                P::leaf("DataTablePtr", "TWeakObjectPtr<UIcarusDataTable>", 0),
                P::leaf("RowName", "FName", 8),
                P::leaf("DataTableName", "FName", 16),
            ],
        ),
    ];
    all.extend(fields);
    P::structure(struct_name, struct_name, 0, all)
}

/// A `FFastArraySerializer` property whose only own field is its item array.
fn fast_array(name: &str, struct_name: &str, offset: i32, item: P) -> P {
    let skipped = |name: &str, cpp: &str, offset| P {
        rep_skip: true,
        ..P::leaf(name, cpp, offset)
    };
    let items = item.name.clone();
    P::structure(
        name,
        struct_name,
        offset,
        vec![
            skipped("ArrayReplicationKey", "int32", 84),
            skipped("DeltaFlags", "EFastArraySerializerDeltaFlags", 256),
            P::array(&items, 264, item),
        ],
    )
}

#[test]
fn bestiary_manager_component_matches_the_controller_subobject_checksum() {
    // M-1092: `UBestiaryManagerComponent`'s two replicated fast arrays fold
    // over `UActorComponent` to the checksum of the controller's
    // `BestiaryManagerComponent` subobject, so its net cache (bound 7) indexes
    // `Client_RequestBestiaryData` and `Server_ReceiveBestiaryData`.
    let actor_component = class_checksum(
        0,
        &[
            P::leaf("bIsActive", "uint8", 138),
            P::leaf("bReplicates", "uint8", 136),
        ],
    );
    let beast = fast_array_item(
        "BestiaryFastArrayItem",
        ("BestiaryRowHandle", "BestiaryDataRowHandle"),
        vec![P::leaf("PointScore", "int32", 36)],
    );
    let fish = fast_array_item(
        "FishDataFastArrayItem",
        ("FishRow", "FishDataRowHandle"),
        vec![
            P::leaf("MaxQuality", "int32", 36),
            P::leaf("MaxWeight", "int32", 40),
            P::leaf("MaxLength", "int32", 44),
            P::leaf("CaughtCount", "int32", 48),
        ],
    );
    let own = [
        fast_array(
            "BeastEntries",
            "BestiaryFastArray",
            192,
            P {
                name: "BestiaryEntries".to_owned(),
                ..beast
            },
        ),
        fast_array(
            "FishEntries",
            "FishDataFastArray",
            536,
            P {
                name: "FishEntries".to_owned(),
                ..fish
            },
        ),
    ];
    assert_eq!(class_checksum(actor_component, &own), 2_451_266_650);
}
