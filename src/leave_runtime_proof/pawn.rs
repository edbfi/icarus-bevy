use super::*;
use crate::experimental::{
    actor_channel::{ActorChannelCodec, read_actor_bunch_body},
    pawn_open::character_state_handle as h,
    rep_layout::{RepLayoutCodec, RepValue, read_rep_properties},
    session_open::quantize_spawn_vector,
};

impl PrivateLeaveProof {
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn pawn_composed(
        &self,
        epoch: u64,
        projection: PlayerProjection,
        location: [f32; 3],
        vitals: CharacterVitals,
        channel: u32,
        pawn_guid: u32,
        payload: &Payload,
        plan: &PawnOpenPlan,
        order: ByteOrder,
    ) -> Result<()> {
        self.access(|s| {
            let (reserved_epoch, record) = s
                .evidence
                .reserved
                .as_ref()
                .ok_or(error(ProofFailure::Order))?;
            require(
                *reserved_epoch == epoch
                    && !s.evidence.activated
                    && !s.evidence.pawn
                    && s.evidence.bound,
                ProofFailure::Order,
            )?;
            require(
                Record::from_projection(record.key()?, projection, &record.growth_name)? == *record
                    && location.map(f32::to_bits)
                        == record
                            .location
                            .map(|v| (f64::from_bits(v) as f32).to_bits())
                    && vital_values(vitals) == record.vitals
                    && vitals.biome_row == record.biome
                    && vitals.growth_row == record.growth,
                ProofFailure::Mismatch,
            )?;
            require(payload.bytes().len() <= 64 * 1024, ProofFailure::Limit)?;
            let actor = ActorChannelCodec {
                engine_net_version: plan.engine_net_version,
                exports_enabled: false,
                has_level: true,
                order,
                limits: Default::default(),
            };
            let body = read_actor_bunch_body(&mut payload.reader(), true, &actor)
                .map_err(|_| error(ProofFailure::Mismatch))?;
            let spawn = body.spawn.as_ref().ok_or(error(ProofFailure::Mismatch))?;
            require(
                spawn.actor.guid == pawn_guid
                    && spawn.dynamic.as_ref().and_then(|v| v.location)
                        == Some(
                            quantize_spawn_vector(location)
                                .map_err(|_| error(ProofFailure::Mismatch))?,
                        ),
                ProofFailure::Mismatch,
            )?;
            let block = body
                .blocks
                .get(1 + plan.actor_state)
                .ok_or(error(ProofFailure::Mismatch))?;
            let codec = RepLayoutCodec {
                actor,
                checksums: false,
                limits: Default::default(),
            };
            let stream = read_rep_properties(
                &mut block.payload.reader(),
                &plan.content.character_state_layout,
                &codec,
            )
            .map_err(|_| error(ProofFailure::Mismatch))?;
            for (handle, value) in vitals.values() {
                require(
                    stream.entries.iter().filter(|e| e.handle == handle).count() == 1
                        && stream
                            .entries
                            .iter()
                            .any(|e| e.handle == handle && e.value == value),
                    ProofFailure::Mismatch,
                )?;
            }
            require(
                stream.entries.iter().any(|e| {
                    e.handle == h::CURRENT_ALIVE_STATE
                        && e.value == RepValue::Enum(u32::from(vitals.health < 1))
                }),
                ProofFailure::Mismatch,
            )?;
            s.candidate = Some(Candidate {
                epoch,
                channel,
                pawn: pawn_guid,
                bits: payload.bit_len(),
                bytes: payload.bytes().to_vec(),
            });
            Ok(())
        })
    }
    pub(crate) fn pawn_admitted(
        &self,
        epoch: u64,
        channel: u32,
        pawn_guid: u32,
        first: u32,
        last: u32,
    ) -> Result<()> {
        self.access(|s| {
            let p = s.candidate.take().ok_or(error(ProofFailure::Order))?;
            require(
                p.epoch == epoch && p.channel == channel && p.pawn == pawn_guid,
                ProofFailure::Mismatch,
            )?;
            s.dir
                .publish(&format!("phase-{}-pawn.bin", s.descriptor.phase), &p.bytes)?;
            s.emit(Event::Pawn {
                epoch,
                channel,
                pawn: pawn_guid,
                first,
                last,
                payload_bits: p.bits,
                payload_sha256: reader::digest(&p.bytes),
            })
        })
    }
}
