use icarus_bevy::experimental::actor_channel::{
    ActorChannelCodec, ActorChannelLimits, GuidKind, PackageMapState,
};
use icarus_bevy::experimental::archive::ByteOrder;
use icarus_bevy::experimental::session_open::{
    ComponentExport, NetGuidCache, PathSegment, PlayerControllerProfile, encode_exports,
};

fn codec() -> ActorChannelCodec {
    ActorChannelCodec {
        engine_net_version: 17,
        exports_enabled: false,
        has_level: true,
        order: ByteOrder::Little,
        limits: ActorChannelLimits::default(),
    }
}

/// Authored identifiers in the shape of F-778 (not release data).
fn profile() -> PlayerControllerProfile {
    PlayerControllerProfile {
        archetype: vec![
            PathSegment::new("Default__TestController_C", 0x1234_5678, false),
            PathSegment::new("/Game/Test/TestController", 0, false),
        ],
        level: vec![
            PathSegment::new("PersistentLevel", 0, true),
            PathSegment::new("TestMap", 0, true),
            PathSegment::new("/Game/Maps/TestMap", 0, true),
        ],
        components: ["Alpha", "Beta", "Gamma"]
            .iter()
            .enumerate()
            .map(|(i, name)| ComponentExport {
                name: (*name).to_owned(),
                checksum: 100 + i as u32,
            })
            .collect(),
    }
}

#[test]
fn guid_numbering_follows_the_captured_policy() {
    let profile = profile();
    let mut cache = NetGuidCache::new();
    assert_eq!(cache.register_level(&profile.level), 3);
    let (guids, references) = cache.player_controller_exports(&profile).unwrap();
    assert_eq!(guids.actor, 2);
    assert_eq!(guids.level, 3);
    assert_eq!(guids.archetype, 9);
    assert_eq!(guids.player_state, 4);
    assert_eq!(guids.components, vec![6, 8, 10]);
    assert_eq!(
        references.iter().map(|r| r.guid).collect::<Vec<_>>(),
        vec![9, 3, 6, 8, 10]
    );
    // The archetype's outer is its package (11), whose outer is null.
    let archetype = references[0].summary();
    assert_eq!(archetype.path_units, Some(25));
    let package = archetype.outer.unwrap();
    assert!(package.export && package.checksum && !package.no_load);
    assert_eq!(package.outer.unwrap().kind, GuidKind::Null);
    // Components: no_load, checksummed, outer = the actor with bare flags only.
    let component = references[2].summary();
    assert!(component.no_load && component.checksum);
    let outer = component.outer.unwrap();
    assert_eq!(outer.kind, GuidKind::Dynamic);
    assert!(outer.export && outer.no_load && outer.checksum && outer.path_units.is_none());
}

#[test]
fn encoded_exports_decode_back_into_the_package_map() {
    let profile = profile();
    let mut cache = NetGuidCache::new();
    cache.register_level(&profile.level);
    let (_, references) = cache.player_controller_exports(&profile).unwrap();
    let payload = encode_exports(&references, &codec()).unwrap();
    let mut map = PackageMapState::new();
    let decoded = map
        .consume_export_prefix(&mut payload.reader(), &codec())
        .unwrap();
    assert_eq!(decoded, references);
    assert_eq!(
        map.resolve_full_path(9).as_deref(),
        Some("/Game/Test/TestController.Default__TestController_C")
    );
    assert_eq!(
        map.resolve_full_path(3).as_deref(),
        Some("/Game/Maps/TestMap.TestMap.PersistentLevel")
    );
    assert_eq!(map.get(6).unwrap().checksum, Some(100));
    assert_eq!(map.resolve_outer_guid(8), Some(2));
}

#[test]
fn a_connection_exports_each_static_path_once() {
    let profile = profile();
    let mut cache = NetGuidCache::new();
    cache.register_level(&profile.level);
    let first = cache.export_static(&profile.archetype).unwrap();
    let again = cache.export_static(&profile.archetype).unwrap();
    assert!(first.export.is_some());
    assert_eq!(again.guid, first.guid);
    assert!(again.export.is_none());
}
