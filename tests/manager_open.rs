//! World manager opens (F-859) from the built-in release plans, pinned to
//! the bit lengths and SHA-256 digests of the opens M-707 captured (taken
//! from the capture, not from the composer), composed in the capture's GUID
//! state.
use icarus_bevy::experimental::{
    archive::ByteOrder,
    manager_open::{ManagerSlot, compose_manager_open},
    packet::Payload,
    release::ReleaseProfile,
    session_open::{ActorKey, ConnectionExports, NetGuidCache, PathSegment},
};
use sha2::{Digest, Sha256};

/// One captured manager open and the GUIDs assigned before it.
struct Captured {
    slot: ManagerSlot,
    channel: u32,
    actor: u32,
    archetype: u32,
    /// The archetype's outer package (native managers only).
    outer: Option<u32>,
    first_component: u32,
    exports_bits: usize,
    exports_sha256: &'static str,
    body_bits: usize,
    body_sha256: &'static str,
}

const CAPTURED: [Captured; 3] = [
    Captured {
        slot: ManagerSlot::Radiation,
        channel: 124,
        actor: 78,
        archetype: 817,
        outer: Some(23),
        first_component: 3078,
        exports_bits: 761,
        exports_sha256: "a736b2f1dfaa77e8961558cf72f05dc3e9db84aefaa0776969b500e1bb756277",
        body_bits: 204,
        body_sha256: "208ead61343d15e8d5b403f6330109de72af1267a999701768b238dcd0eef7dc",
    },
    Captured {
        slot: ManagerSlot::WorldTalent,
        channel: 125,
        actor: 76,
        archetype: 819,
        outer: Some(23),
        first_component: 3082,
        exports_bits: 777,
        exports_sha256: "f1010d7635f366293b1ca853859e6644130309033e9846253d48f59e0fa6e882",
        body_bits: 204,
        body_sha256: "9de9da14ab53bfe8100b8951565cb304c66e728deec2278d3a3f5f2e77f0c83f",
    },
    Captured {
        slot: ManagerSlot::Disaster,
        channel: 127,
        actor: 86,
        archetype: 823,
        outer: None,
        first_component: 3090,
        exports_bits: 1089,
        exports_sha256: "0025c91eae09f73d408464dda887d39c436c675e5d2d945265423744357de7bd",
        body_bits: 104,
        body_sha256: "3a8be5ffbbaffc0ae7cd03cbd32c937fd8b89731e74db1fb0d4b6c1a9e8e12ca",
    },
];

fn sha256(payload: &Payload) -> String {
    Sha256::digest(payload.bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Assign static GUIDs until the next one is `next`.
fn burn_static_to(cache: &mut NetGuidCache, next: u32) {
    for index in 0.. {
        let probe = cache.assign_static(&[PathSegment::new(&format!("burn{index}"), 0, false)]);
        if probe + 2 >= next {
            assert_eq!(probe + 2, next, "static GUID {next} unreachable");
            return;
        }
    }
}

#[test]
fn manager_opens_match_the_captured_bit_lengths_and_digests() {
    let plans = ReleaseProfile::release_3_0_31()
        .manager_open_plans("/Game/Maps/Terrain_016_OLY/Terrain_016")
        .unwrap();
    for captured in &CAPTURED {
        let slot = captured.slot;
        let plan = &plans
            .iter()
            .find(|(candidate, _)| *candidate == slot)
            .unwrap()
            .1;
        // The level and the native archetype's outer package were exported
        // and acked; the actor's GUID was assigned at the GameState.
        let mut cache = NetGuidCache::new();
        let mut exports = ConnectionExports::new();
        cache.export_static(&mut exports, &plan.level).unwrap();
        if let Some(outer) = captured.outer {
            burn_static_to(&mut cache, outer);
            let held = cache
                .export_static(&mut exports, &plan.archetype[1..])
                .unwrap();
            assert_eq!(held.guid, outer);
        }
        burn_static_to(&mut cache, captured.archetype);
        while cache.assign_dynamic() + 2 < captured.actor {}
        let key = ActorKey::World(1);
        assert_eq!(cache.actor_guid(key), captured.actor);
        while cache.assign_dynamic() + 2 < captured.first_component {}
        exports.commit_bunch(1);
        exports.received_ack(1);
        let open = compose_manager_open(
            &mut cache,
            &mut exports,
            plan,
            key,
            captured.channel,
            ByteOrder::Little,
        )
        .unwrap();
        let open_exports = open.exports.unwrap();
        assert_eq!(open_exports.bit_len(), captured.exports_bits, "{slot:?}");
        assert_eq!(sha256(&open_exports), captured.exports_sha256, "{slot:?}");
        assert_eq!(open.bunch.payload.bit_len(), captured.body_bits, "{slot:?}");
        assert_eq!(
            sha256(&open.bunch.payload),
            captured.body_sha256,
            "{slot:?}"
        );
    }
}
