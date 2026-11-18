//! Bounded operator snapshot of the last peer removed by a client control close.

use super::{PeerEntry, PendingProfileExchange, ProfileKind, located};
use serde::Serialize;
use std::time::Instant;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct ClientControlClose {
    pub reason: Option<u32>,
    pub phase: PlayerEntryPhase,
    /// Absent when connected-player records are disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub own_record_ack: Option<OwnRecordAck>,
    /// Absent when frozen-movement release is disabled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub frozen_movement_release: Option<FrozenReleaseAtClose>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PlayerEntryPhase {
    Unregistered,
    AwaitingBegin,
    BeginWaitingForPlayerState,
    AwaitingUserProfile,
    AwaitingCharacterProfile,
    AwaitingInitialisation,
    AwaitingCompletion,
    AwaitingFinalisation,
    Finalised,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OwnRecordAck {
    Unacknowledged,
    InClosePacket,
    Earlier,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FrozenReleaseAtClose {
    pub state: FrozenReleaseState,
    /// A copy entered outgoing transport, including before a pending retry.
    pub sent: bool,
    /// Pawn channel condition while a release send or retry is still due or scheduled.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pawn: Option<ReleasePawnState>,
    pub relocation_held: bool,
    pub deferred: bool,
    /// Whether the live peer reached the release pass after its first own-record ACK.
    /// Absent until that ACK.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_pass_reached: Option<bool>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FrozenReleaseState {
    Unscheduled,
    AwaitingDelay,
    Due,
    PendingSend,
    AwaitingAck,
    Acked,
    Failed,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ReleasePawnState {
    Unavailable,
    ReliablePending,
    Settled,
}

impl PeerEntry {
    pub(super) fn entry_phase(&self) -> PlayerEntryPhase {
        use PlayerEntryPhase::*;
        if self.player.is_none() {
            Unregistered
        } else if self.profiles.finalised {
            Finalised
        } else if self.profiles.completed {
            AwaitingFinalisation
        } else if self.profiles.has_loadout && self.profiles.has_bestiary {
            AwaitingCompletion
        } else if self.profiles.initialisation_requested
            || matches!(
                self.pending_profile_exchange,
                Some(PendingProfileExchange::Initialisation)
            )
        {
            AwaitingInitialisation
        } else if self.profiles.outstanding == Some(ProfileKind::Character)
            || matches!(
                self.pending_profile_exchange,
                Some(
                    PendingProfileExchange::Request(ProfileKind::Character)
                        | PendingProfileExchange::Publish(ProfileKind::Character, _)
                )
            )
        {
            AwaitingCharacterProfile
        } else if self.character_slot.is_some() {
            AwaitingUserProfile
        } else if self
            .initial_opens
            .as_ref()
            .is_some_and(|pending| pending.begin_slot.is_some())
        {
            BeginWaitingForPlayerState
        } else {
            AwaitingBegin
        }
    }

    pub(super) fn own_record_ack_at_close(&self, acked_now: bool) -> OwnRecordAck {
        if acked_now {
            OwnRecordAck::InClosePacket
        } else if self.own_record_acked {
            OwnRecordAck::Earlier
        } else {
            OwnRecordAck::Unacknowledged
        }
    }

    pub(super) fn frozen_release_at_close(
        &self,
        pawn_channel: Option<u32>,
        now: Instant,
    ) -> FrozenReleaseAtClose {
        use FrozenReleaseState::*;
        let release = &self.frozen_release;
        let state = if release.acked_at.is_some() {
            Acked
        } else if release.failed {
            Failed
        } else if !release.in_flight.is_empty() {
            AwaitingAck
        } else if release.pending {
            PendingSend
        } else if release.awaiting_delay(now) {
            AwaitingDelay
        } else if release.idle() && release.due(now) {
            Due
        } else {
            Unscheduled
        };
        let pawn = matches!(state, AwaitingDelay | Due | PendingSend).then(|| match pawn_channel {
            Some(channel) if self.has_open_pawn(channel) => {
                if self.peer.actor_channel_settled(channel) {
                    ReleasePawnState::Settled
                } else {
                    ReleasePawnState::ReliablePending
                }
            }
            _ => ReleasePawnState::Unavailable,
        });
        FrozenReleaseAtClose {
            state,
            sent: release.sent_at.is_some(),
            pawn,
            relocation_held: state == Due && located::release_held(self, now),
            deferred: release.deferred,
            service_pass_reached: release
                .record_acked_at
                .map(|_| release.service_pass_reached),
        }
    }
}
