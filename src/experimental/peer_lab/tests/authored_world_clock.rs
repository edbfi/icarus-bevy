//! Authored tick-derived wire time; no original daylight or client claim.
use super::super::super::{
    actor_channel::read_actor_bunch_body,
    archive::ByteOrder,
    game_state_open::handle as h,
    packet::{BitWriter, Bunch, Payload},
    rep_layout::{RepLayoutCodec, append_bits, read_rep_properties},
    sequencing::PacketSequencer,
    time_of_day::RateCurve,
};
use super::*;
use crate::{
    authored_world_bootstrap::AuthoredClockConfig,
    config::ServerConfig,
    native_players::{PlayerContext, PlayerSnapshot},
    persistence::SnapshotStore,
    simulation::{SNAPSHOT_SCHEMA_PLAYERS, SimulationClock, WorldSnapshot, build_app, capture},
};

fn plan() -> AuthoredClockPlan {
    AuthoredClockConfig {
        initial_minute: 480,
        seconds_per_day: 3600,
        tick_hz: 30,
    }
    .prepare()
    .unwrap()
}

fn enabled(tick: u64) -> PeerLab {
    let mut lab = prospect_refresh_lab();
    lab.enable_authored_time_of_day(plan(), tick).unwrap();
    lab
}

fn opened(
    lab: &mut PeerLab,
) -> (
    UdpSocket,
    SocketAddrV4,
    PacketSequencer,
    BTreeMap<u32, RepValue>,
) {
    let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
    socket.connect(lab.local_addr().unwrap()).unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let (address, mut client) = handshake_peer_on(lab, &socket);
    assert!(matches!(
        lab.send_game_state_open(address),
        InitialOpenOutcome::Committed(_)
    ));
    let bunches = deliver_bunches(lab, &socket, &mut client, |_| true);
    let values = decode_open(lab, &bunches);
    (socket, address, client, values)
}

fn decode_open(lab: &PeerLab, bunches: &[Bunch]) -> BTreeMap<u32, RepValue> {
    decode_state(lab, bunches, true)
}

fn decode_state(lab: &PeerLab, bunches: &[Bunch], open: bool) -> BTreeMap<u32, RepValue> {
    let session = lab.game_state_open.as_ref().unwrap();
    let mut payload = BitWriter::new();
    for bunch in bunches.iter().filter(|bunch| {
        bunch.header.channel_index == session.channel_index && !bunch.header.has_package_map_exports
    }) {
        append_bits(&mut payload, &bunch.payload);
        if !bunch.header.partial || bunch.header.partial_final {
            break;
        }
    }
    let codec = RepLayoutCodec {
        actor: actor_codec(session.plan.engine_net_version, ByteOrder::Little),
        checksums: false,
        limits: Default::default(),
    };
    let body = read_actor_bunch_body(
        &mut Payload::from_writer(payload).reader(),
        open,
        &codec.actor,
    )
    .unwrap();
    read_rep_properties(
        &mut body.blocks[0].payload.reader(),
        &session.plan.layout,
        &codec,
    )
    .unwrap()
    .entries
    .into_iter()
    .map(|entry| (entry.handle, entry.value))
    .collect()
}

fn assert_time(values: &BTreeMap<u32, RepValue>, tick: u64) {
    assert_eq!(
        values.get(&h::TIME_OF_DAY),
        Some(&RepValue::Float(plan().sample(tick).to_bits()))
    );
    assert_eq!(
        values.get(&h::SECONDS_PER_GAME_DAY),
        Some(&RepValue::Int(3600))
    );
}

fn open_restored_snapshot(snapshot: &WorldSnapshot) -> BTreeMap<u32, RepValue> {
    let mut app = build_app(&ServerConfig::default(), snapshot).unwrap();
    app.insert_resource(enabled(0));
    app.add_systems(bevy::prelude::Update, poll_peer_lab);
    // Exercise the same pre-poll system using build_app's restored owner,
    // before opening the authored peer. No fixed step has completed yet.
    app.world_mut().run_schedule(bevy::prelude::Update);
    let mut lab = app.world_mut().remove_resource::<PeerLab>().unwrap();
    opened(&mut lab).3
}

#[test]
fn authored_clock_opens_at_sample_and_midnight_sends_explicit_zero() {
    let mut lab = enabled(27_000);
    let (socket, _address, mut client, values) = opened(&mut lab);
    assert_time(&values, 27_000);
    lab.sample_authored_time_of_day(72_000);
    lab.game_state_open.as_mut().unwrap().updated_at = None;
    lab.update_game_state();
    let changed = receive_game_state_update(&socket, &lab, Some(&mut client));
    assert_time(&changed, 72_000);
    assert_eq!(
        lab.game_state_open
            .as_ref()
            .unwrap()
            .sampled_clock
            .unwrap()
            .minute
            .to_bits(),
        0
    );
}

#[test]
fn declared_large_day_length_reaches_the_wire_without_float_rounding() {
    let mut lab = prospect_refresh_lab();
    let plan = AuthoredClockConfig {
        initial_minute: 480,
        seconds_per_day: i32::MAX,
        tick_hz: 30,
    }
    .prepare()
    .unwrap();
    lab.enable_authored_time_of_day(plan, u64::MAX).unwrap();
    let (_, _, _, values) = opened(&mut lab);
    assert_eq!(values[&h::SECONDS_PER_GAME_DAY], RepValue::Int(i32::MAX));
    assert_eq!(
        values[&h::TIME_OF_DAY],
        RepValue::Float(plan.sample(u64::MAX).to_bits())
    );
}

#[test]
fn unchanged_completed_tick_does_not_advance_with_wall_time_polling() {
    let mut lab = enabled(27_000);
    let (socket, _address, mut client, values) = opened(&mut lab);
    assert_time(&values, 27_000);
    let session = lab.game_state_open.as_mut().unwrap();
    session.clock_at = Instant::now() - Duration::from_secs(120);
    session.updated_at = None;
    lab.started = Instant::now() - Duration::from_secs(120);
    lab.update_game_state();
    let world = receive_game_state_update(&socket, &lab, Some(&mut client));
    assert!(
        world.contains_key(&h::WORLD_TIME_SECONDS),
        "world-seconds ownership is unchanged"
    );
    assert!(!world.contains_key(&h::TIME_OF_DAY));
    assert_eq!(
        lab.game_state_open
            .as_ref()
            .unwrap()
            .sampled_clock
            .unwrap()
            .minute,
        plan().sample(27_000)
    );
    for _ in 0..3 {
        lab.sample_authored_time_of_day(27_000);
        lab.game_state_open.as_mut().unwrap().updated_at = None;
        lab.update_game_state();
    }
    assert_eq!(
        lab.game_state_open
            .as_ref()
            .unwrap()
            .sampled_clock
            .unwrap()
            .minute,
        plan().sample(27_000)
    );
}

#[test]
fn ring_pressure_retries_latest_sample_and_nak_repairs_latest_value() {
    let mut lab = enabled(0);
    lab.config.packet.sent_capacity = 16;
    let (socket, address, mut client, _) = opened(&mut lab);
    let capacity = lab.peers[&address].peer.config().sent_capacity;
    let pending = lab.peers[&address]
        .peer
        .sequencer()
        .notify()
        .sent_records()
        .len();
    for _ in pending..capacity {
        let datagram = lab
            .peers
            .get_mut(&address)
            .unwrap()
            .peer
            .ack_packet()
            .unwrap();
        assert!(lab.send(&datagram, address));
    }
    for tick in [9_000, 18_000, 27_000] {
        lab.sample_authored_time_of_day(tick);
        lab.game_state_open.as_mut().unwrap().updated_at = None;
        lab.update_game_state();
        assert_eq!(
            lab.peers[&address].game_state.as_ref().unwrap().sent[&h::TIME_OF_DAY],
            RepValue::Float(480f32.to_bits())
        );
    }
    deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    lab.game_state_open.as_mut().unwrap().updated_at = None;
    lab.update_game_state();
    let lost = deliver_bunches(&mut lab, &socket, &mut client, |_| false);
    let changed = decode_state(&lab, &lost, false);
    assert_time(&changed, 27_000);
    // A client NAK traversed packet notification and made the latest sample
    // due. The retry may coalesce it to a newer completed tick.
    assert!(
        !lab.peers[&address]
            .game_state
            .as_ref()
            .unwrap()
            .sent
            .contains_key(&h::TIME_OF_DAY)
    );
    lab.sample_authored_time_of_day(36_000);
    lab.game_state_open.as_mut().unwrap().updated_at = None;
    lab.update_game_state();
    let delivered = deliver_bunches(&mut lab, &socket, &mut client, |_| true);
    let repaired = decode_state(&lab, &delivered, false);
    assert_time(&repaired, 36_000);
}

#[test]
fn sampled_and_monotonic_modes_are_exclusive_and_clockless_wire_stays_ordinary() {
    let legacy = || TimeOfDayClock::new(RateCurve::constant(60.0).unwrap(), 600.0);
    let mut sampled = enabled(0);
    assert!(sampled.enable_time_of_day(legacy()).is_err());
    let mut ordinary = prospect_refresh_lab();
    ordinary.enable_time_of_day(legacy()).unwrap();
    assert!(ordinary.enable_authored_time_of_day(plan(), 0).is_err());
    let mut clockless = prospect_refresh_lab();
    let (_, _, _, values) = opened(&mut clockless);
    assert!(!values.contains_key(&h::TIME_OF_DAY));
    assert!(!values.contains_key(&h::SECONDS_PER_GAME_DAY));
}

#[test]
fn poll_samples_owner_before_publication_and_missing_owner_or_rate_closes_admission() {
    for missing in ["clock", "rate"] {
        let mut app = App::new();
        app.insert_resource(enabled(0));
        app.insert_resource(SimulationClock { tick: 27_000 });
        let duration = ServerConfig::default().tick_interval();
        app.insert_resource(Time::<Fixed>::from_duration(duration));
        app.add_systems(bevy::prelude::Update, poll_peer_lab);
        app.update();
        assert_eq!(
            app.world()
                .resource::<PeerLab>()
                .game_state_open
                .as_ref()
                .unwrap()
                .sampled_clock
                .unwrap()
                .minute,
            plan().sample(27_000)
        );
        let address = app.world().resource::<PeerLab>().local_addr().unwrap();
        let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        socket
            .send_to(&super::super::super::handshake::initial_request(), address)
            .unwrap();
        if missing == "clock" {
            app.world_mut().remove_resource::<SimulationClock>();
        } else {
            app.world_mut()
                .resource_mut::<Time<Fixed>>()
                .set_timestep(Duration::from_millis(50));
        }
        app.update();
        let lab = app.world().resource::<PeerLab>();
        assert!(!lab.is_admitting());
        assert_eq!(
            lab.stats.peers_created, 0,
            "queued packet was not processed"
        );
    }
}

#[test]
fn committed_tick_reopen_and_offline_ticks_drive_next_decoded_open() {
    let context = PlayerContext::new(
        "authored-clock".encode_utf16().collect(),
        PeerLabAdmission::default().map_url.into(),
        &ReleaseProfile::release_3_0_32().pin,
        [17; 32],
    )
    .unwrap();
    let mut registry = crate::native_player_runtime::PlayerRegistry::new(
        PlayerSnapshot::empty(context.clone()),
        &context,
        8,
    )
    .unwrap();
    let key = crate::native_players::PlayerKey::new(7, 3).unwrap();
    let checkpoint = crate::native_players::PlayerCheckpoint {
        location_cm: [100.0, 200.0, 300.0],
        vitals: super::super::super::pawn_open::CharacterVitals {
            health: 37,
            max_health: 100,
            stamina: 80,
            max_stamina: 100,
            oxygen: 70,
            max_oxygen: 100,
            water: 60,
            max_water: 100,
            food: 50,
            max_food: 100,
            biome_row: 0,
            growth_row: 0,
        },
    };
    registry.reserve(key, 1, checkpoint).unwrap();
    registry.activate(1).unwrap();
    let mut snapshot = WorldSnapshot {
        schema_version: SNAPSHOT_SCHEMA_PLAYERS,
        tick: 27_000,
        native_players: Some(registry.snapshot()),
        ..Default::default()
    };
    let directory = tempfile::tempdir().unwrap();
    let mut store = SnapshotStore::open(directory.path()).unwrap();
    store.save(&snapshot).unwrap();
    let first = open_restored_snapshot(&snapshot);
    let loaded = store.load().unwrap().unwrap().snapshot;
    let same = open_restored_snapshot(&loaded);
    assert_eq!(first[&h::TIME_OF_DAY], same[&h::TIME_OF_DAY]);
    assert_time(&same, 27_000);
    let mut app = build_app(&ServerConfig::default(), &loaded).unwrap();
    for _ in 0..3 {
        app.world_mut().run_schedule(bevy::prelude::FixedUpdate);
    }
    snapshot = capture(app.world_mut()).unwrap();
    assert_eq!(snapshot.tick, 27_003);
    store.save(&snapshot).unwrap();
    let reopened = store.load().unwrap().unwrap().snapshot;
    assert_time(&open_restored_snapshot(&reopened), 27_003);
    // Corrupt only the current generation; the existing store recovers its
    // preceding complete generation and samples that recovered tick.
    let receipt = store
        .save(&WorldSnapshot {
            tick: 27_006,
            ..reopened
        })
        .unwrap();
    std::fs::write(
        directory
            .path()
            .join(format!("snapshot-{}.json", receipt.generation)),
        b"corrupt",
    )
    .unwrap();
    let recovered = store.load().unwrap().unwrap();
    assert!(recovered.recovered_previous);
    assert_eq!(recovered.snapshot.tick, 27_003);
    assert_eq!(
        recovered
            .snapshot
            .native_players
            .as_ref()
            .unwrap()
            .projection(key)
            .unwrap()
            .checkpoint,
        checkpoint
    );
    assert_time(&open_restored_snapshot(&recovered.snapshot), 27_003);
}
