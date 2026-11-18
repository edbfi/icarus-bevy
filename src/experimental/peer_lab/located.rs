//! EXPERIMENTAL, both default off: a player's accepted move location saved
//! into their checkpoint (`network.checkpoint_saved_locations`) and a
//! returning player's early pawn moved to its stored location by a movement
//! correction (`network.checkpoint_location_correction`).
//!
//! Saving takes the session location F-1128 already accepted (no base, a
//! movement mode, finite, inside the world bound, within the speed bound of
//! the previous one) and commits it through the registry's ordinary checkpoint
//! edit rules ([`save_player_locations`]): at a staged authored leave just
//! before its cut-off, lab-wide every [`LOCATION_SAVE_INTERVAL`], and when the
//! network owner is torn down. The original persists the owner's reported
//! position at save (F-1135).
//!
//! Relocation answers the newest client move after the checkpoint is bound
//! with one unreliable field-30 correction carrying that move's timestamp, the
//! stored location raised by the saved-location clearance (F-1139), zero
//! velocity and mode Falling, and nothing else, as the original's placement
//! correction (F-1134, F-1132; layout F-1204). While it is pending no move is
//! acknowledged as good, stored or speed-checked; a world move near the target
//! applies it, and a bounded number of lost corrections or a bounded time
//! gives it up, counted, leaving the pawn where its client reports it. The
//! frozen-movement release waits for it, at most [`RELOCATION_RELEASE_HOLD`]
//! past its due time (F-1134: correction first, unfreeze after). Unqualified
//! with a stock client.
use super::super::character_move::{
    MOVEMENT_MODE_FALLING, MoveCorrection, compose_move_correction_block, move_response_codec,
};
use super::*;
use crate::native_player_runtime::{LocationSave, LocationSaveOutcome, save_player_locations};
use crate::spawn_placement::saved_location_target;

/// Lab-wide interval between periodic location saves: a session that ends
/// without an authored leave (a timeout, a client close) keeps the location
/// of the last pass. Authored, not the original's.
pub const LOCATION_SAVE_INTERVAL: Duration = Duration::from_secs(10);
/// Shortest interval between two corrections of one relocation: room for
/// the client's answer to the previous one to arrive. Authored.
pub const RELOCATION_RESEND_INTERVAL: Duration = Duration::from_secs(1);
/// Corrections one relocation sends before it gives up. Authored.
pub const MAX_RELOCATION_CORRECTIONS: u8 = 5;
/// Longest a relocation stays pending after its checkpoint is bound,
/// whether or not moves arrive. Authored.
pub const RELOCATION_TIMEOUT: Duration = Duration::from_secs(30);
/// How close a client move must be to the target to apply the relocation:
/// the 50 cm clearance plus about 0.7 s of free fall. Authored.
pub const RELOCATION_TOLERANCE_CM: f64 = 300.0;
/// Longest the frozen-movement release waits, past its due time, for a
/// pending relocation. Authored.
pub const RELOCATION_RELEASE_HOLD: Duration = Duration::from_secs(5);

/// Saved checkpoint locations ([`PeerLab::enable_checkpoint_saved_locations`]);
/// counts only, never locations.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct SavedLocationCounts {
    /// Checkpoint edits that stored a new session location.
    pub saved: u64,
    /// Of those, the ones taken for a staged authored leave before its cut-off.
    pub saved_at_leave: u64,
    /// Eligible sessions whose save the registry refused (no activated
    /// attachment, a leave already cut off, an invalid edit or a failed
    /// world preflight); nothing changed for them.
    pub refused: u64,
}

/// Live relocations ([`PeerLab::enable_checkpoint_location_correction`]);
/// counts only, never locations or timestamps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CheckpointRelocationCounts {
    /// Returning players whose stored location differed from their opened
    /// pawn and was within the placement bound.
    pub started: u64,
    /// Corrections sent unreliable, first sends and resends.
    pub corrections_sent: u64,
    /// Held back by drainable transport pressure until ACK/NAK progress.
    pub corrections_deferred: u64,
    /// Composition or framing refusals that cannot drain (each gives the
    /// relocation up).
    pub correction_errors: u64,
    /// A client move within [`RELOCATION_TOLERANCE_CM`] of the target
    /// arrived after a correction.
    pub applied: u64,
    /// Given up (attempts, time or an error); also counted in
    /// `checkpoint_locations_unapplied`.
    pub failed: u64,
    /// World moves received while pending, neither stored nor checked.
    pub moves_held: u64,
    /// Relocations pending now.
    pub pending: usize,
    /// Frozen-movement releases due now but held for a pending relocation.
    pub releases_held: usize,
}

/// One session's pending relocation; fixed size, dropped with the session.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(super) struct Relocation {
    /// The stored location raised by the saved-location clearance (F-1139).
    pub(super) target: [f32; 3],
    /// When the checkpoint was bound and the relocation became pending.
    pub(super) since: Instant,
    /// A client move arrived since then, or since the last correction.
    pub(super) due: bool,
    /// Corrections sent so far.
    pub(super) sent: u8,
    pub(super) last_sent_at: Option<Instant>,
}

impl Relocation {
    pub(super) fn pending(target: [f32; 3], since: Instant) -> Self {
        Self {
            target,
            since,
            due: false,
            sent: 0,
            last_sent_at: None,
        }
    }

    /// Whether a frozen-movement release due at `due_at` is still held.
    fn holds_release(due_at: Option<Instant>, now: Instant) -> bool {
        due_at.is_some_and(|due| now < due + RELOCATION_RELEASE_HOLD)
    }
}

/// Whether `entry`'s frozen-movement release, due at `due_at`, waits for its
/// pending relocation at `now` (always false without one).
pub(super) fn release_held(entry: &PeerEntry, now: Instant) -> bool {
    entry.pawn_moves.relocation.is_some()
        && Relocation::holds_release(entry.frozen_release.due_at, now)
}

/// A world move (no base, a movement mode) received while a relocation is
/// pending: it applies the relocation when a correction has been sent and the
/// move lies within [`RELOCATION_TOLERANCE_CM`] of the target (and passes the
/// world checks); that client-reported location becomes the session location,
/// from which F-1128's bounds resume. Otherwise it is neither stored nor
/// speed-checked: the jump to the target must not be refused.
pub(super) fn relocating_move(
    state: &mut PawnMoves,
    stats: &mut PeerLabStats,
    location: [f64; 3],
    now: Instant,
) {
    let Some(relocation) = state.relocation else {
        return;
    };
    let distance = location
        .iter()
        .zip(relocation.target.map(f64::from))
        .map(|(from, to)| (to - from) * (to - from))
        .sum::<f64>()
        .sqrt();
    if relocation.sent > 0
        && check_move_location(None, location, now).is_ok()
        && distance <= RELOCATION_TOLERANCE_CM
    {
        state.relocation = None;
        state.location = Some((location, now));
        state.counts.positions_accepted = state.counts.positions_accepted.saturating_add(1);
        bump(&mut stats.pawn_move_positions_accepted);
        bump(&mut stats.checkpoint_relocation.applied);
    } else {
        bump(&mut stats.checkpoint_relocation.moves_held);
    }
}

/// The save one session offers: an authenticated, joined, activated session
/// with an accepted location and no pending relocation.
fn location_save(entry: &PeerEntry) -> Option<LocationSave> {
    if !(entry.auth_accepted
        && entry.auth_request.is_some()
        && entry.admission.state == ClientLoginState::Joined
        && entry.checkpoint_active)
        || entry.pawn_moves.relocation.is_some()
    {
        return None;
    }
    let (location_cm, _) = entry.pawn_moves.location?;
    let slot = entry.profiles.character_slot?;
    if entry.character_slot != Some(slot) {
        return None;
    }
    Some(LocationSave {
        key: PlayerKey::new(entry.auth_identity?, slot).ok()?,
        epoch: entry.checkpoint_epoch?,
        location_cm,
    })
}

impl PeerLab {
    /// EXPERIMENTAL, default off (`network.checkpoint_saved_locations`). Save
    /// each activated player's last accepted move location (F-1128) as their
    /// checkpoint's location: before a staged authored leave's cut-off, every
    /// [`LOCATION_SAVE_INTERVAL`] and at teardown
    /// ([`Self::bridge_saved_locations`]), never while a relocation is
    /// pending. Requires player checkpoints and pawn move acknowledgements,
    /// before peers; the private leave proof refuses it. Session behaviour
    /// only: no player input, digest or schema changes.
    pub fn enable_checkpoint_saved_locations(&mut self) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "saved checkpoint locations must precede peers"
        );
        ensure!(
            self.private_leave_proof.is_none(),
            "the private leave proof refuses saved checkpoint locations"
        );
        ensure!(
            self.checkpoint_initial_vitals.is_some(),
            "saved checkpoint locations require player checkpoints"
        );
        ensure!(
            self.pawn_move_acks,
            "saved checkpoint locations require pawn move acknowledgements"
        );
        self.checkpoint_saved_locations = true;
        Ok(())
    }

    /// Whether accepted move locations are saved into checkpoints.
    pub fn has_checkpoint_saved_locations(&self) -> bool {
        self.checkpoint_saved_locations
    }

    /// EXPERIMENTAL, default off (`network.checkpoint_location_correction`).
    /// With the early checkpoint pawn, a returning player whose stored
    /// location differs from where the pawn opened is moved there by a
    /// movement correction (module docs) instead of being counted in
    /// `checkpoint_locations_unapplied`. Requires the early pawn and pawn move
    /// acknowledgements, before peers; the private leave proof refuses it.
    pub fn enable_checkpoint_location_correction(&mut self) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "checkpoint location correction must precede peers"
        );
        // Named before the early pawn, which the proof also refuses.
        ensure!(
            self.private_leave_proof.is_none(),
            "the private leave proof refuses checkpoint location correction"
        );
        ensure!(
            self.checkpoint_early_pawn,
            "checkpoint location correction requires the early checkpoint pawn"
        );
        ensure!(
            self.pawn_move_acks,
            "checkpoint location correction requires pawn move acknowledgements"
        );
        self.checkpoint_location_correction = true;
        Ok(())
    }

    /// Whether a returning early pawn is moved by a correction.
    pub fn has_checkpoint_location_correction(&self) -> bool {
        self.checkpoint_location_correction
    }

    /// One saved-location pass at an exclusive world boundary, after the
    /// poll and before the authored leave bridge, so a leave staged in this
    /// poll is cut off with the location it carries. No-op when off.
    pub fn bridge_saved_locations(&mut self, world: &mut bevy::prelude::World) {
        self.save_locations(world, Instant::now(), false);
    }

    /// Commit every session's latest location now (teardown).
    pub(crate) fn flush_saved_locations(&mut self, world: &mut bevy::prelude::World) {
        self.save_locations(world, Instant::now(), true);
    }

    /// Collect at most one save per eligible session: every session when the
    /// periodic interval has passed (or `flush`), otherwise only sessions with
    /// a staged leave not yet cut off. Bounded by the peer table.
    pub(super) fn save_locations(
        &mut self,
        world: &mut bevy::prelude::World,
        now: Instant,
        flush: bool,
    ) {
        if !self.checkpoint_saved_locations || self.checkpoint_initial_vitals.is_none() {
            return;
        }
        if crate::network::authored_clock_runtime_health(world).is_err()
            || crate::native_player_runtime::consumption_runtime_health(world).is_err()
        {
            return;
        }
        let leaving = |entry: &PeerEntry| {
            entry
                .controller_leave
                .as_ref()
                .is_some_and(controller_leave::Operation::awaiting_cutoff)
        };
        let periodic = flush
            || self
                .last_location_save_at
                .is_none_or(|at| now.saturating_duration_since(at) >= LOCATION_SAVE_INTERVAL);
        // Nothing due: no registry work at all on this pass.
        if !periodic && !self.peers.values().any(leaving) {
            return;
        }
        {
            let Some(mut registry) = world.get_resource_mut::<PlayerRegistry>() else {
                return;
            };
            // The leave bridge's eligibility cut, first.
            self.reconcile_player_authority(&mut registry);
        }
        if periodic {
            self.last_location_save_at = Some(now);
        }
        let mut addresses: Vec<_> = self.peers.keys().copied().collect();
        addresses.sort_unstable();
        let mut saves = Vec::new();
        let mut at_leave = Vec::new();
        for address in addresses {
            let entry = &self.peers[&address];
            let leaving = leaving(entry);
            if !(periodic || leaving) {
                continue;
            }
            if let Some(save) = location_save(entry) {
                saves.push(save);
                at_leave.push(leaving);
            }
        }
        if saves.is_empty() {
            return;
        }
        let counts = &mut self.stats.checkpoint_saved_locations;
        match save_player_locations(world, &saves) {
            Ok(outcomes) => {
                for (outcome, leaving) in outcomes.into_iter().zip(at_leave) {
                    match outcome {
                        LocationSaveOutcome::Saved => {
                            bump(&mut counts.saved);
                            if leaving {
                                bump(&mut counts.saved_at_leave);
                            }
                        }
                        LocationSaveOutcome::Unchanged => {}
                        LocationSaveOutcome::Refused => bump(&mut counts.refused),
                    }
                }
            }
            Err(_) => {
                counts.refused = counts.refused.saturating_add(saves.len() as u64);
            }
        }
    }

    /// At the early pawn's reservation, a returning player's stored location
    /// `stored` differs from where the pawn opened: start its relocation, or
    /// count it unapplied (option off, or a target outside the bound).
    pub(super) fn start_relocation(
        &mut self,
        stored: [f32; 3],
        now: Instant,
    ) -> Option<Relocation> {
        let target = self
            .checkpoint_location_correction
            .then(|| saved_location_target(stored))
            .flatten();
        match target {
            Some(target) => {
                bump(&mut self.stats.checkpoint_relocation.started);
                Some(Relocation::pending(target, now))
            }
            None => {
                bump(&mut self.stats.checkpoint_locations_unapplied);
                None
            }
        }
    }

    /// The relocation part of the move response pass ([`Self::retry_pawn_move_acks`]):
    /// sessions due a correction under the acknowledgement's own preconditions
    /// (a new move, the pawn channel open and settled, no transport deferral,
    /// [`MOVE_ACK_INTERVAL`] since the last acknowledgement) and
    /// [`RELOCATION_RESEND_INTERVAL`] since the last correction; and sessions
    /// whose relocation ran out of time or attempts.
    pub(super) fn retry_relocations(&mut self, channel: u32, now: Instant) {
        if !self.checkpoint_location_correction {
            return;
        }
        let mut corrections = Vec::new();
        let mut exhausted = Vec::new();
        for (address, entry) in &self.peers {
            let moves = &entry.pawn_moves;
            let Some(relocation) = moves.relocation else {
                continue;
            };
            if now.saturating_duration_since(relocation.since) >= RELOCATION_TIMEOUT {
                exhausted.push(*address);
                continue;
            }
            let ready = relocation.due
                && !moves.deferred
                && relocation.last_sent_at.is_none_or(|at| {
                    now.saturating_duration_since(at) >= RELOCATION_RESEND_INTERVAL
                })
                && moves
                    .last_ack_at
                    .is_none_or(|at| now.saturating_duration_since(at) >= MOVE_ACK_INTERVAL)
                && entry.controller_startup_pawn_guid.is_some()
                && entry.peer.has_actor_channel(channel)
                && !entry.peer.actor_channel_closing(channel)
                && entry.peer.actor_channel_settled(channel);
            if !ready {
                continue;
            }
            // The last correction had a full interval and the client's newest
            // move is still away from the target.
            if relocation.sent >= MAX_RELOCATION_CORRECTIONS {
                exhausted.push(*address);
            } else {
                corrections.push(*address);
            }
        }
        for address in exhausted {
            self.fail_relocation(address);
        }
        for address in corrections {
            self.send_relocation_correction(address, now);
        }
    }

    /// Give a pending relocation up: counted, the pawn left where its
    /// client reports it, ordinary acknowledgements and bounds resume.
    fn fail_relocation(&mut self, address: SocketAddrV4) {
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        let Some(relocation) = entry.pawn_moves.relocation.take() else {
            return;
        };
        bump(&mut self.stats.checkpoint_relocation.failed);
        bump(&mut self.stats.checkpoint_locations_unapplied);
        bevy::log::warn!(
            "connection {:?}: pawn relocation given up after {} correction(s); the pawn stays where its client reports it",
            entry.connection_id,
            relocation.sent
        );
    }

    /// One correction for `address`'s pending relocation, answering its
    /// newest move: unreliable and alone in its block, as acknowledgements.
    fn send_relocation_correction(&mut self, address: SocketAddrV4, now: Instant) {
        let Some((plan, channel, _)) = &self.pawn_open else {
            return;
        };
        let (channel, engine) = (*channel, plan.engine_net_version);
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        let (Some(timestamp), Some(relocation)) = (
            entry.pawn_moves.latest_timestamp,
            entry.pawn_moves.relocation,
        ) else {
            return;
        };
        let correction = MoveCorrection {
            timestamp,
            location: relocation.target,
            velocity: [0.0; 3],
            rotation: None,
            base: None,
            bone: None,
            mode: Some(MOVEMENT_MODE_FALLING),
            base_relative: false,
            root_motion: None,
        };
        let codec = move_response_codec(engine, entry.peer.connection_byte_order());
        let sent = compose_move_correction_block(&correction, &codec)
            .and_then(|payload| entry.peer.actor_update_datagrams(channel, payload));
        match sent {
            Ok(datagrams) => {
                let moves = &mut entry.pawn_moves;
                if let Some(pending) = &mut moves.relocation {
                    pending.sent = pending.sent.saturating_add(1);
                    pending.due = false;
                    pending.last_sent_at = Some(now);
                }
                // The correction answered the newest move.
                moves.unacked = false;
                moves.last_correction_at = Some(now);
                bump(&mut self.stats.checkpoint_relocation.corrections_sent);
                entry.ack_pending = false;
                for (datagram, _) in &datagrams {
                    self.send(datagram, address);
                }
            }
            Err(error) => {
                if transport_capacity_can_drain(entry, &error, channel) {
                    entry.pawn_moves.deferred = true;
                    bump(&mut self.stats.checkpoint_relocation.corrections_deferred);
                } else {
                    bump(&mut self.stats.checkpoint_relocation.correction_errors);
                    let failed = entry.peer.is_failed();
                    self.fail_relocation(address);
                    if failed {
                        self.remove_peer(address);
                        bump(&mut self.stats.peers_failed);
                    }
                }
            }
        }
    }

    /// The relocation counts with their live gauges.
    pub(super) fn relocation_counts(&self) -> CheckpointRelocationCounts {
        let now = Instant::now();
        let mut counts = self.stats.checkpoint_relocation;
        for entry in self.peers.values() {
            if entry.pawn_moves.relocation.is_some() {
                counts.pending += 1;
                let release = &entry.frozen_release;
                counts.releases_held += usize::from(
                    release.idle()
                        && release.due_at.is_some_and(|due| due <= now)
                        && release_held(entry, now),
                );
            }
        }
        counts
    }
}
