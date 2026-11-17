//! Private proof sink with authored loopback and local writer only.
use super::*;
use crate::leave_runtime_proof::{PrivateLeaveProof, ProofFailure, ProofJoinKind};
use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};

fn proof() -> (tempfile::TempDir, PrivateLeaveProof) {
    use std::io::Write;
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let id = directory.path().file_name().unwrap().to_str().unwrap();
    let path = directory.path().join("descriptor.json");
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)
        .unwrap();
    write!(file, "{{\"format\":1,\"case_id\":{id:?},\"phase\":1,\"output\":\"phase-1.jsonl\",\"predecessor\":null,\"exit_seal\":null}}").unwrap();
    let proof = PrivateLeaveProof::open_authored_fixture(
        &path,
        directory.path(),
        "11111111111111111111111111111111",
    )
    .unwrap();
    proof.observe_load(None).unwrap();
    proof
        .observe_opened(&WorldSnapshot {
            schema_version: SNAPSHOT_SCHEMA_PLAYERS,
            native_players: Some(registry().snapshot()),
            ..Default::default()
        })
        .unwrap();
    (directory, proof)
}

#[test]
fn private_proof_actual_projection_completion_and_detach_are_bound() {
    let (directory, proof) = proof();
    let mut f = Fixture::with_proof(true, Some(proof.clone()));
    assert!(proof.first_pawn_admitted(f.epoch));
    assert!(f.registry().is_activated(f.epoch));
    let (_store, mut writer) = f.writer();
    f.send(Request::Normal(Some(Kind::ReturnToCharacterSelect)));
    proof.health().expect("accepted phase-one intent");
    f.bridge();
    proof.health().expect("exact registry cutoff");
    f.save(&mut writer);
    proof
        .health()
        .expect("exact writer readback and durable receipt");
    f.bridge();
    assert!(!proof.phase_ready().unwrap());
    assert!(!f.deliver(true).is_empty());
    f.bridge();
    assert!(proof.phase_ready().unwrap());
    assert!(!f.lab.peers.contains_key(&f.address));
    assert!(f.registry().active_epochs().is_empty());
    let journal = std::fs::read_to_string(directory.path().join("phase-1.jsonl")).unwrap();
    for tag in [
        "Reserved",
        "Bound",
        "Pawn",
        "Activated",
        "Intent",
        "Cutoff",
        "Submitted",
        "Readback",
        "Durable",
        "Completion",
        "PrefixAcked",
        "Detached",
        "Ready",
    ] {
        assert!(journal.contains(tag), "missing {tag}");
    }
    assert!(
        std::fs::metadata(directory.path().join("phase-1-pawn.bin"))
            .unwrap()
            .len()
            <= 64 * 1024
    );
}

#[test]
fn private_proof_cancel_or_abandoned_peer_never_completes_and_latches_dispatch() {
    for cancel in [false, true] {
        let (_directory, proof) = proof();
        let mut f = Fixture::with_proof(true, Some(proof.clone()));
        if cancel {
            f.send(Request::Normal(Some(Kind::ReturnToCharacterSelect)));
            f.bridge();
            f.send(Request::Cancel);
        } else {
            f.lab.remove_peer(f.address);
        }
        assert!(proof.health().is_err());
        let before = f.lab.stats.clone();
        f.lab
            .controller_call(f.address, ControllerCall::FriendsList);
        f.bridge();
        f.lab
            .bridge_player_checkpoints(&mut f.app.world_mut().resource_mut::<PlayerRegistry>());
        assert_eq!(f.lab.stats, before);
        assert!(proof.phase_ready().is_err());
    }
}

#[test]
fn private_proof_positive_ack_keeps_the_fixed_prefix_through_a_lost_predecessor() {
    let (_directory, proof) = proof();
    check_completion_gap(Fixture::with_proof(true, Some(proof.clone())));
    assert!(proof.phase_ready().unwrap());
}

#[test]
fn private_proof_phase_one_refuses_other_save_intending_types_before_cutoff() {
    for kind in [Kind::Quit, Kind::ReturnToTitlescreen] {
        let (_directory, proof) = proof();
        let mut f = Fixture::with_proof(true, Some(proof.clone()));
        f.send(Request::Normal(Some(kind)));
        f.bridge();
        assert!(proof.health().is_err());
        assert_eq!(f.phase(), None);
        assert_eq!(f.registry().leave_counts().cutoffs_accepted, 0);
        assert!(f.lab.peers[&f.address].controller_leave.is_none());
    }
}

#[test]
fn private_proof_close_ack_is_abandonment_even_when_it_acks_completion() {
    let (_directory, proof) = proof();
    let mut f = Fixture::with_proof(true, Some(proof.clone()));
    let (_store, mut writer) = f.writer();
    f.send(Request::Normal(Some(Kind::ReturnToCharacterSelect)));
    proof.health().expect("accepted phase-one intent");
    f.bridge();
    proof.health().expect("exact registry cutoff");
    f.save(&mut writer);
    proof
        .health()
        .expect("exact writer readback and durable receipt");
    f.bridge();
    let entry = f.lab.peers.get_mut(&f.address).unwrap();
    let bytes = entry
        .peer
        .reliable_actor_close(entry.player_controller_channel.unwrap(), 0)
        .unwrap();
    assert!(f.lab.send(&bytes, f.address));
    f.deliver(true);
    assert_eq!(proof.health().unwrap_err().0, ProofFailure::Abandoned);
    f.bridge();
    assert!(proof.phase_ready().is_err());
    assert!(f.lab.peers.contains_key(&f.address));
}

#[test]
fn private_proof_wire_admission_exhaustion_is_terminal_before_second_player_mutation() {
    use super::super::super::private_join_budget::{packet, peer};
    let (_directory, proof) = proof();
    let mut lab = lab();
    let selected = ReleaseProfile::fixture("3.0.36");
    lab.config.local_network_version = Some(selected.pin.network_checksum);
    lab.player_controller_open = selected
        .player_controller_open_plan(PeerLabAdmission::default().map_url, [0.; 3])
        .unwrap();
    lab.enable_authored_controller_leave(plan()).unwrap();
    lab.enable_private_leave_proof(proof.clone()).unwrap();
    let (one, first, mut client_one, mut raw_one) = peer(&mut lab);
    let (two, second, mut client_two, mut raw_two) = peer(&mut lab);
    for address in [first, second] {
        // This helper starts after Login/Welcome. Configured PlayerState
        // composition still requires the identity retained by that Login.
        lab.peers.get_mut(&address).unwrap().login_unique_id = Some(UniqueNetId {
            kind: NetIdType::Named(ArchiveText::text("NULL").unwrap()),
            value: NetIdValue::Text(ArchiveText::text("AUTHORED-LOGIN").unwrap()),
        });
    }
    let bytes = packet(&lab, &mut client_one, &mut raw_one, &[ControlMessage::Join]);
    receive_authored_at(&mut lab, &one, &bytes, Instant::now());
    proof.health().unwrap();
    assert!(lab.peers[&first].player.is_some());
    let bytes = packet(&lab, &mut client_two, &mut raw_two, &[ControlMessage::Join]);
    receive_authored_at(&mut lab, &two, &bytes, Instant::now());
    assert!(proof.health().is_err());
    assert_eq!(lab.stats.joins_succeeded, 1);
    assert_eq!(lab.next_player_id, 257);
    assert!(lab.peers[&second].player.is_none());
    assert!(!lab.peers[&second].admission.has_player_controller);
}

#[test]
fn private_proof_first_pawn_pressure_prevents_activation_and_checks_actual_retry_arguments() {
    for mismatch in [false, true] {
        let (directory, proof) = proof();
        let (mut lab, mut registry) = (lab(), registry());
        lab.config.packet.sent_capacity = 16;
        let selected = ReleaseProfile::fixture("3.0.36");
        lab.config.local_network_version = Some(selected.pin.network_checksum);
        lab.player_controller_open = selected
            .player_controller_open_plan(PeerLabAdmission::default().map_url, [0.; 3])
            .unwrap();
        lab.enable_authored_controller_leave(plan()).unwrap();
        lab.enable_private_leave_proof(proof.clone()).unwrap();
        registry.enable_private_leave_proof(proof.clone()).unwrap();
        proof.joined(1, ProofJoinKind::Join).unwrap();
        let (socket, address, mut client, _) = peer(&mut lab, 1, ACCOUNT, true);
        identify(&mut lab, &socket, &mut client, address, 3, 3);
        let entry = lab.peers.get_mut(&address).unwrap();
        entry.profiles.completed = true;
        entry.profiles.friend_ids_ready = true;
        entry.friends_query_pending = false;
        let capacity = entry.peer.config().sent_capacity;
        let pending = entry.peer.sequencer().notify().sent_records().len();
        for _ in pending..capacity {
            let bytes = lab
                .peers
                .get_mut(&address)
                .unwrap()
                .peer
                .ack_packet()
                .unwrap();
            assert!(lab.send(&bytes, address));
        }
        lab.bridge_player_checkpoints(&mut registry);
        let epoch = lab.peers[&address].checkpoint_epoch.unwrap();
        let reserved = registry.projection(epoch).unwrap();
        assert!(!proof.first_pawn_admitted(epoch));
        assert!(registry.activated_epochs().is_empty());
        registry
            .consume_activated(ConsumptionRates::new(1, 1, 1).unwrap())
            .unwrap();
        assert_eq!(registry.projection(epoch).unwrap(), reserved);
        assert!(!directory.path().join("phase-1-pawn.bin").exists());
        deliver_bunches(&mut lab, &socket, &mut client, |_| true);
        if mismatch {
            lab.peers
                .get_mut(&address)
                .unwrap()
                .placement
                .as_mut()
                .unwrap()
                .location = [0.; 3];
        }
        lab.retry_initial_opens();
        lab.bridge_player_checkpoints(&mut registry);
        if mismatch {
            assert_eq!(proof.health().unwrap_err().0, ProofFailure::Mismatch);
            assert!(registry.activated_epochs().is_empty());
            assert!(!directory.path().join("phase-1-pawn.bin").exists());
        } else {
            proof.health().unwrap();
            assert!(proof.first_pawn_admitted(epoch));
            assert!(registry.is_activated(epoch));
            assert!(directory.path().join("phase-1-pawn.bin").exists());
        }
    }
}

/// The proof's first-pawn receipt binds an open composed from the bound
/// reservation (vitals, location and alive state). The early pawn opens before
/// any reservation exists, so the two refuse each other in either order.
#[test]
fn early_checkpoint_pawn_and_private_proof_refuse_each_other() {
    let leave_lab = || {
        let mut lab = lab();
        let selected = ReleaseProfile::fixture("3.0.36");
        lab.config.local_network_version = Some(selected.pin.network_checksum);
        lab.player_controller_open = selected
            .player_controller_open_plan(PeerLabAdmission::default().map_url, [0.0; 3])
            .unwrap();
        lab.enable_authored_controller_leave(plan()).unwrap();
        lab
    };
    let (_first_directory, first) = proof();
    let mut lab = leave_lab();
    lab.enable_private_leave_proof(first.clone()).unwrap();
    assert!(
        lab.enable_checkpoint_early_pawn()
            .unwrap_err()
            .to_string()
            .contains("refuses the early checkpoint pawn")
    );
    assert!(!lab.has_checkpoint_early_pawn());
    first.health().unwrap();
    let (_second_directory, second) = proof();
    let mut lab = leave_lab();
    lab.enable_checkpoint_early_pawn().unwrap();
    assert!(
        lab.enable_private_leave_proof(second.clone())
            .unwrap_err()
            .to_string()
            .contains("refuses the early checkpoint pawn")
    );
    assert!(lab.private_leave_proof.is_none());
    assert_eq!(lab.private_join_remaining, None);
    second.health().unwrap();
}
