//! Explicit authored normal-leave policy. Static codecs alone never install it.
use super::super::{
    release::ControllerLeavePlan,
    rep_rpc::{ControllerLeaveRequest, LeaveProspectSessionType},
    session_open::compose_controller_leave_completed_update,
};
use super::*;
use crate::native_player_runtime::LeavePhase;

/// Aggregate native transport transitions. ACK means delivery of the reliable
/// prefix, never client application or menu success. Retained after departure.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize)]
pub struct ControllerLeaveCounts {
    pub intents_accepted: u64,
    pub cancel_suppressions: u64,
    pub completions_admitted: u64,
    pub completion_prefix_acked: u64,
}

pub(super) struct Operation {
    pub(super) connection: CheckpointConnection,
    key: PlayerKey,
    session_type: LeaveProspectSessionType,
    cutoff: bool,
    /// Cancellation retains the exact checkpoint obligation and freeze. It
    /// suppresses only a completion not yet admitted to reliable transport.
    pub(super) suppressed: bool,
    pub(super) receipt: InitialisationReceipt,
    pub(super) retry_ready: bool,
}

impl PeerLab {
    /// Whether an authored leave policy is installed.
    pub fn has_authored_controller_leave(&self) -> bool {
        self.authored_controller_leave.is_some()
    }

    /// Install an explicit authored policy separately from codec qualification.
    /// A caller must opt in separately from static codec qualification; no
    /// active pin or default grants dispatch or client qualification.
    pub fn enable_authored_controller_leave(&mut self, plan: ControllerLeavePlan) -> Result<()> {
        ensure!(
            self.peers.is_empty() && self.peak_peers == 0,
            "authored controller leave must precede admission"
        );
        ensure!(
            self.checkpoint_initial_vitals.is_some(),
            "authored controller leave requires player checkpoints"
        );
        ensure!(
            self.config.local_network_version == Some(plan.network_checksum()),
            "authored controller leave differs from session network checksum"
        );
        let controller = self
            .player_controller_open
            .as_ref()
            .context("authored controller leave requires a controller plan")?;
        plan.validate_controller_open(controller)?;
        ensure!(
            self.authored_controller_leave.is_none(),
            "authored controller leave already installed"
        );
        self.authored_controller_leave = Some(plan);
        Ok(())
    }

    pub(super) fn stage_authored_controller_leave(
        &mut self,
        address: SocketAddrV4,
        request: ControllerLeaveRequest,
    ) {
        if !self.private_proof_ready() {
            return;
        }
        if self.authored_controller_leave.is_none() {
            return;
        }
        let Some(entry) = self.peers.get_mut(&address) else {
            return;
        };
        if !(entry.auth_accepted
            && entry.auth_request.is_some()
            && entry.admission.state == ClientLoginState::Joined
            && entry.checkpoint_active)
        {
            return;
        }
        if let Some(operation) = &mut entry.controller_leave {
            if request != ControllerLeaveRequest::Normal(Some(operation.session_type))
                && let Some(proof) = &self.private_leave_proof
            {
                let _ = proof.fail(crate::leave_runtime_proof::ProofFailure::Abandoned);
            }
            if request == ControllerLeaveRequest::Cancel
                && operation.receipt.channel.is_none()
                && !operation.suppressed
            {
                operation.suppressed = true;
                bump(&mut self.stats.controller_leave.cancel_suppressions);
            }
            // The first normal intent owns the cutoff/type; duplicate or
            // conflicting calls never extend its deadline or replace it.
            return;
        }
        let ControllerLeaveRequest::Normal(Some(
            session_type @ (LeaveProspectSessionType::Quit
            | LeaveProspectSessionType::ReturnToCharacterSelect
            | LeaveProspectSessionType::ReturnToTitlescreen),
        )) = request
        else {
            if let Some(proof) = &self.private_leave_proof {
                let _ = proof.fail(crate::leave_runtime_proof::ProofFailure::Mismatch);
            }
            return;
        };
        let (Some(connection_id), Some(epoch), Some(account), Some(slot)) = (
            entry.connection_id,
            entry.checkpoint_epoch,
            entry.auth_identity,
            entry.profiles.character_slot,
        ) else {
            return;
        };
        if entry.character_slot != Some(slot) {
            return;
        }
        let Ok(key) = PlayerKey::new(account, slot) else {
            return;
        };
        if let Some(proof) = &self.private_leave_proof
            && proof
                .normal_intent(connection_id, key, epoch, session_type)
                .is_err()
        {
            self.admitting = false;
            return;
        }
        entry.controller_leave = Some(Operation {
            connection: CheckpointConnection {
                address,
                connection_id,
                epoch,
            },
            key,
            session_type,
            cutoff: false,
            suppressed: false,
            receipt: InitialisationReceipt::default(),
            retry_ready: true,
        });
        bump(&mut self.stats.controller_leave.intents_accepted);
    }

    /// Apply accepted intents at an exclusive boundary before fixed ticks,
    /// including updates with zero ticks. The registry and its exact writer
    /// receipt alone establish durability; reliable admission and ACK are
    /// separate stages. Bounded by the admitted peer count, one operation each.
    pub fn bridge_controller_leaves(&mut self, registry: &mut PlayerRegistry) {
        if !self.private_proof_ready() {
            return;
        }
        if self.authored_controller_leave.is_none() {
            return;
        }
        self.reconcile_player_authority(registry);
        if !self.private_proof_ready() {
            return;
        }
        let mut addresses: Vec<_> = self.peers.keys().copied().collect();
        addresses.sort_unstable();
        let mut departing = Vec::new();
        let mut outgoing = Vec::new();
        for address in addresses {
            if !self.private_proof_ready() {
                return;
            }
            let Some(entry) = self.peers.get_mut(&address) else {
                continue;
            };
            let Some(mut operation) = entry.controller_leave.take() else {
                continue;
            };
            let epoch = operation.connection.epoch;
            let eligible = operation.connection.address == address
                && entry.connection_id == Some(operation.connection.connection_id)
                && entry.checkpoint_epoch == Some(epoch)
                && entry.auth_accepted
                && entry.auth_request.is_some()
                && entry.admission.state == ClientLoginState::Joined
                && entry.auth_identity == Some(operation.key.account)
                && entry.profiles.character_slot == Some(operation.key.slot);
            if !eligible {
                if let Some(proof) = &self.private_leave_proof {
                    let _ = proof.abandoned(epoch);
                }
                registry.detach(epoch);
                continue;
            }
            if !operation.cutoff {
                let Some(projection) = registry.projection(epoch) else {
                    continue;
                };
                if registry
                    .request_leave(
                        operation.key,
                        epoch,
                        projection.revision,
                        Instant::now(),
                        Duration::from_secs(60),
                    )
                    .is_err()
                {
                    if let Some(proof) = &self.private_leave_proof {
                        let _ = proof.fail(crate::leave_runtime_proof::ProofFailure::Mismatch);
                    }
                    continue;
                }
                operation.cutoff = true;
            }
            if operation.receipt.settled || operation.receipt.abandoned {
                let success = operation.receipt.settled && !operation.receipt.abandoned;
                if !success && let Some(proof) = &self.private_leave_proof {
                    let _ = proof.abandoned(epoch);
                }
                departing.push((address, epoch, success));
                continue;
            }
            let durable = registry
                .leave_status(operation.key, epoch)
                .is_some_and(|status| matches!(status.phase, LeavePhase::Durable { .. }));
            if durable
                && !operation.suppressed
                && operation.receipt.channel.is_none()
                && let Some(channel) = entry.player_controller_channel
                && entry.peer.has_actor_channel(channel)
                && !entry.peer.actor_channel_closing(channel)
                && std::mem::take(&mut operation.retry_ready)
            {
                let plan = self
                    .authored_controller_leave
                    .as_ref()
                    .expect("enabled leave plan");
                let codec = RepLayoutCodec {
                    actor: actor_codec(
                        plan.engine_net_version(),
                        entry.peer.connection_byte_order(),
                    ),
                    checksums: false,
                    limits: Default::default(),
                };
                let sent = (|| {
                    let payload = compose_controller_leave_completed_update(
                        plan,
                        operation.session_type,
                        &codec,
                    )?;
                    let sent = entry
                        .peer
                        .reliable_actor_update_datagrams(channel, payload, &[])?;
                    Ok::<_, anyhow::Error>(sent)
                })();
                match sent {
                    Ok(sent) => {
                        // An ACK can cover a queued completion before an earlier
                        // reliable bunch arrives. Keep repairing predecessors
                        // until the whole prefix through completion settles.
                        operation.receipt = InitialisationReceipt::through(&entry.peer, channel)
                            .expect("committed leave controller channel");
                        if let Some(proof) = &self.private_leave_proof
                            && proof
                                .completion_admitted(
                                    epoch,
                                    channel,
                                    operation.receipt.first,
                                    operation.receipt.last,
                                )
                                .is_err()
                        {
                            entry.controller_leave = Some(operation);
                            self.admitting = false;
                            return;
                        }
                        bump(&mut self.stats.controller_leave.completions_admitted);
                        entry.ack_pending = false;
                        outgoing.push((address, sent));
                    }
                    Err(error) if transport_capacity_can_drain(entry, &error, channel) => (),
                    Err(_) => {
                        if let Some(proof) = &self.private_leave_proof {
                            let _ = proof.abandoned(epoch);
                        }
                        departing.push((address, epoch, false));
                    }
                }
            }
            entry.controller_leave = Some(operation);
        }
        for (address, sent) in outgoing {
            if !self.private_proof_ready() {
                return;
            }
            self.send_all(&sent, address);
        }
        for (address, epoch, success) in departing {
            registry.detach(epoch);
            self.private_expected_detach = success.then_some(epoch);
            self.remove_peer(address);
            self.private_expected_detach = None;
            if success && let Some(proof) = &self.private_leave_proof {
                let _ = proof.detached(
                    epoch,
                    !registry.active_epochs().contains(&epoch),
                    !self.peers.contains_key(&address),
                );
            }
        }
        // A lost completion destination suppresses delivery, never durability.
        // Pending/failed obligations survive and continue to block reattachment.
        registry.settle_detached_durable_leaves();
    }
}
