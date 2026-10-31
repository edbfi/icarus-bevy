//! Independent classic Steam session-ticket verification and CM bookkeeping.
//!
//! Protocol references:
//! <https://github.com/SteamRE/SteamKit/blob/24131c045655ddaa1e0e60880b2b1af28f69e89c/Resources/Structs/steam3_appticket.hsl>
//! <https://github.com/DoctorMcKay/node-steam-gameserver/blob/d6a999b131e13ebc3b03539342daf7217bb47844/components/appauth.js>
//! AuthList registers the session prefix; it does not validate ownership. The
//! signed ownership section is verified locally, then a correlated asynchronous
//! TicketAuthComplete establishes service acceptance. Later failure revokes it.
//! No socket or admission policy is hidden in this module.

use std::{
    collections::{BTreeMap, BTreeSet},
    fmt,
    time::{Duration, Instant},
};

use anyhow::{Context, Result, ensure};
use prost::Message as _;
use ring::signature::{RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY, RsaPublicKeyComponents};

use crate::steam_fetch::{
    frame::{Packet, emsg},
    proto::{AuthTicket, ClientAuthList, ClientAuthListAck, ClientTicketAuthComplete},
};

/// Authored bound matching the auth component's 1024 hexadecimal characters.
pub const MAX_TICKET_BYTES: usize = 512;
const AUTH_PREFIX_BYTES: usize = 52;
const SIGNATURE_BYTES: usize = 128;
const MAX_CM_AUTH_BYTES: usize = 16 * 1024;

// Steam's public System key, exponent 17. Public cryptographic parameter only;
// source: https://github.com/DoctorMcKay/node-steam-crypto/blob/b568b4a81150353b71d50285540e90a902ce552d/system.pem
const SYSTEM_MODULUS: [u8; 128] = [
    0xdf, 0xec, 0x1a, 0xd6, 0x2c, 0x10, 0x66, 0x2c, 0x17, 0x35, 0x3a, 0x14, 0xb0, 0x7c, 0x59, 0x11,
    0x7f, 0x9d, 0xd3, 0xd8, 0x2b, 0x7a, 0xe3, 0xe0, 0x15, 0xcd, 0x19, 0x1e, 0x46, 0xe8, 0x7b, 0x87,
    0x74, 0xa2, 0x18, 0x46, 0x31, 0xa9, 0x03, 0x14, 0x79, 0x82, 0x8e, 0xe9, 0x45, 0xa2, 0x49, 0x12,
    0xa9, 0x23, 0x68, 0x73, 0x89, 0xcf, 0x69, 0xa1, 0xb1, 0x61, 0x46, 0xbd, 0xc1, 0xbe, 0xbf, 0xd6,
    0x01, 0x1b, 0xd8, 0x81, 0xd4, 0xdc, 0x90, 0xfb, 0xfe, 0x4f, 0x52, 0x73, 0x66, 0xcb, 0x95, 0x70,
    0xd7, 0xc5, 0x8e, 0xba, 0x1c, 0x7a, 0x33, 0x75, 0xa1, 0x62, 0x34, 0x46, 0xbb, 0x60, 0xb7, 0x80,
    0x68, 0xfa, 0x13, 0xa7, 0x7a, 0x8a, 0x37, 0x4b, 0x9e, 0xc6, 0xf4, 0x5d, 0x5f, 0x3a, 0x99, 0xf9,
    0x9e, 0xc4, 0x3a, 0xe9, 0x63, 0xa2, 0xbb, 0x88, 0x19, 0x28, 0xe0, 0xe7, 0x14, 0xc0, 0x42, 0x89,
];

/// Decode the auth component's hexadecimal FString without printing its content.
pub fn decode_ticket_hex(units: &[u16]) -> Result<Vec<u8>> {
    ensure!(
        !units.is_empty() && units.len() <= MAX_TICKET_BYTES * 2 && units.len().is_multiple_of(2),
        "invalid Steam ticket hex length"
    );
    fn nibble(unit: u16) -> Result<u8> {
        Ok(match unit {
            0x30..=0x39 => (unit - 0x30) as u8,
            0x41..=0x46 => (unit - 0x41 + 10) as u8,
            0x61..=0x66 => (unit - 0x61 + 10) as u8,
            _ => anyhow::bail!("invalid Steam ticket hex character"),
        })
    }
    units
        .as_chunks::<2>()
        .0
        .iter()
        .map(|pair| Ok((nibble(pair[0])? << 4) | nibble(pair[1])?))
        .collect()
}

struct Cursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> Cursor<'a> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N]> {
        let end = self
            .offset
            .checked_add(N)
            .context("Steam ticket length overflow")?;
        let bytes = self
            .bytes
            .get(self.offset..end)
            .context("truncated Steam ticket")?;
        self.offset = end;
        Ok(bytes.try_into().expect("checked fixed length"))
    }

    fn u16(&mut self) -> Result<u16> {
        Ok(u16::from_le_bytes(self.take()?))
    }

    fn u32(&mut self) -> Result<u32> {
        Ok(u32::from_le_bytes(self.take()?))
    }

    fn u64(&mut self) -> Result<u64> {
        Ok(u64::from_le_bytes(self.take()?))
    }

    fn licenses(&mut self) -> Result<()> {
        let count = usize::from(self.u16()?);
        ensure!(
            count <= MAX_TICKET_BYTES / 4,
            "Steam license count exceeds bound"
        );
        for _ in 0..count {
            self.u32()?;
        }
        Ok(())
    }
}

/// Local ownership verification only; still requires a successful CM callback.
/// Construction is private so callers cannot bypass signature validation.
pub struct VerifiedTicket {
    steam_id: u64,
    app_id: u32,
    expires_at: u64,
    prefix: [u8; AUTH_PREFIX_BYTES],
    crc: u32,
}

impl fmt::Debug for VerifiedTicket {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("VerifiedTicket([REDACTED])")
    }
}

impl VerifiedTicket {
    /// Admit only the classic GC20/session24/version4 signed format. The caller's
    /// Unix clock is part of this trust boundary and must be checked for errors.
    pub fn verify(bytes: &[u8], claimed_steam_id: u64, app_id: u32, unix_now: u64) -> Result<Self> {
        Self::verify_with_key(
            bytes,
            claimed_steam_id,
            app_id,
            unix_now,
            &SYSTEM_MODULUS,
            &[17],
        )
    }

    fn verify_with_key(
        bytes: &[u8],
        claimed_steam_id: u64,
        app_id: u32,
        unix_now: u64,
        modulus: &[u8],
        exponent: &[u8],
    ) -> Result<Self> {
        ensure!(
            bytes.len() <= MAX_TICKET_BYTES,
            "Steam ticket exceeds byte bound"
        );
        ensure!(
            claimed_steam_id >> 32 == 0x0110_0001 && claimed_steam_id as u32 != 0,
            "Steam ticket requires a public individual identity"
        );
        ensure!(app_id != 0, "Steam ticket requires an app id");
        let mut input = Cursor { bytes, offset: 0 };
        ensure!(input.u32()? == 20, "unsupported Steam GC token length");
        input.u64()?;
        ensure!(
            input.u64()? == claimed_steam_id,
            "Steam token identity mismatch"
        );
        input.u32()?;
        ensure!(
            input.u32()? == 24,
            "unsupported Steam session header length"
        );
        ensure!(
            input.u32()? == 1 && input.u32()? == 2,
            "unsupported Steam session header"
        );
        for _ in 0..4 {
            input.u32()?;
        }
        let prefix: [u8; AUTH_PREFIX_BYTES] = bytes[..input.offset]
            .try_into()
            .expect("fixed validated session prefix");
        let section_len = input.u32()? as usize;
        ensure!(
            section_len == bytes.len() - input.offset,
            "Steam ownership section length mismatch"
        );
        let ownership_start = input.offset;
        let ownership_len = input.u32()? as usize;
        ensure!(
            ownership_len.checked_add(SIGNATURE_BYTES) == Some(section_len),
            "Steam ownership signature length mismatch"
        );
        let ownership_end = ownership_start
            .checked_add(ownership_len)
            .context("Steam ownership length overflow")?;
        ensure!(input.u32()? == 4, "unsupported Steam ownership version");
        ensure!(
            input.u64()? == claimed_steam_id,
            "Steam ownership identity mismatch"
        );
        ensure!(input.u32()? == app_id, "Steam ownership app mismatch");
        input.u32()?;
        input.u32()?;
        input.u32()?;
        let generated_at = u64::from(input.u32()?);
        let expires_at = u64::from(input.u32()?);
        ensure!(
            generated_at <= unix_now && unix_now < expires_at,
            "Steam ownership outside validity window"
        );
        input.licenses()?;
        let dlc_count = usize::from(input.u16()?);
        ensure!(
            dlc_count <= MAX_TICKET_BYTES / 6,
            "Steam DLC count exceeds bound"
        );
        for _ in 0..dlc_count {
            input.u32()?;
            input.licenses()?;
        }
        ensure!(
            input.u16()? == 0,
            "unsupported Steam ownership reserved value"
        );
        ensure!(
            input.offset == ownership_end,
            "Steam ownership body length mismatch"
        );
        let signature = input.take::<SIGNATURE_BYTES>()?;
        ensure!(input.offset == bytes.len(), "trailing Steam ticket bytes");
        RsaPublicKeyComponents {
            n: modulus,
            e: exponent,
        }
        .verify(
            &RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY,
            &bytes[ownership_start..ownership_end],
            &signature,
        )
        .map_err(|_| anyhow::anyhow!("invalid Steam ownership signature"))?;
        Ok(Self {
            steam_id: claimed_steam_id,
            app_id,
            expires_at,
            prefix,
            crc: crc32fast::hash(&prefix),
        })
    }

    pub const fn steam_id(&self) -> u64 {
        self.steam_id
    }
    pub const fn app_id(&self) -> u32 {
        self.app_id
    }
    pub const fn expires_at(&self) -> u64 {
        self.expires_at
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthOutcome {
    Accepted,
    Rejected,
    Revoked,
    TimedOut,
    Expired,
    Disconnected,
}

/// Neutral caller-owned correlation only; no credential or player identifiers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthEvent {
    pub request_id: u64,
    pub outcome: AuthOutcome,
}

struct Session {
    ticket: VerifiedTicket,
    deadline: Instant,
    accepted: bool,
    submitted: bool,
    estate: u32,
}

/// One CM connection's bounded authentication lifecycle. Construct a new instance
/// after reconnect; never reuse this instance after `disconnected`. Request IDs
/// and identity/CRC pairs cannot be reused during its lifetime, so late callbacks
/// cannot authorize a replacement peer. Reaching `max_requests` fails closed.
pub struct AuthSessions {
    app_id: u32,
    max_active: usize,
    max_requests: usize,
    timeout: Duration,
    active: BTreeMap<u64, Session>,
    used_requests: BTreeSet<u64>,
    used_keys: BTreeSet<(u64, u32)>,
    connected: bool,
    dirty: bool,
    sequence: u32,
    ack_sequence: u32,
}

impl AuthSessions {
    /// Authored limits: 1..=64 simultaneous tickets, up to 4096 total requests,
    /// 1..=120 seconds for pending validation. Ownership expiry also applies.
    pub fn new(
        app_id: u32,
        max_active: usize,
        max_requests: usize,
        timeout: Duration,
    ) -> Result<Self> {
        ensure!(app_id != 0, "Steam authentication requires an app id");
        ensure!(
            (1..=64).contains(&max_active),
            "invalid active Steam authentication limit"
        );
        ensure!(
            (max_active..=4096).contains(&max_requests),
            "invalid lifetime Steam authentication limit"
        );
        ensure!(
            (Duration::from_secs(1)..=Duration::from_secs(120)).contains(&timeout),
            "invalid Steam authentication timeout"
        );
        Ok(Self {
            app_id,
            max_active,
            max_requests,
            timeout,
            active: BTreeMap::new(),
            used_requests: BTreeSet::new(),
            used_keys: BTreeSet::new(),
            connected: true,
            dirty: false,
            sequence: 0,
            ack_sequence: 0,
        })
    }

    /// Register pending local state. Call `take_auth_list`, then send it through
    /// the same CM connection whose packets reach `handle_packet`.
    pub fn begin(&mut self, request_id: u64, ticket: VerifiedTicket, now: Instant) -> Result<()> {
        ensure!(self.connected, "Steam authentication connection is closed");
        ensure!(
            ticket.app_id == self.app_id,
            "Steam authentication app mismatch"
        );
        ensure!(
            self.active.len() < self.max_active,
            "Steam authentication capacity exhausted"
        );
        ensure!(
            self.used_requests.len() < self.max_requests,
            "Steam authentication lifetime exhausted"
        );
        ensure!(
            !self.used_requests.contains(&request_id),
            "Steam authentication request already used"
        );
        let key = (ticket.steam_id, ticket.crc);
        ensure!(
            !self.used_keys.contains(&key),
            "Steam authentication ticket already used"
        );
        ensure!(
            self.active
                .values()
                .all(|session| session.ticket.steam_id != ticket.steam_id
                    && session.ticket.crc != ticket.crc),
            "Steam authentication identity or CRC already active"
        );
        let deadline = now
            .checked_add(self.timeout)
            .context("Steam authentication deadline overflow")?;
        self.used_requests.insert(request_id);
        self.used_keys.insert(key);
        self.active.insert(
            request_id,
            Session {
                ticket,
                deadline,
                accepted: false,
                submitted: false,
                estate: 1,
            },
        );
        self.dirty = true;
        Ok(())
    }

    /// Ending a session resends the full list, including this app when empty.
    pub fn end(&mut self, request_id: u64) -> bool {
        let removed = self.active.remove(&request_id).is_some();
        self.dirty |= removed;
        removed
    }

    /// End every session before an orderly CM logoff. Publish the resulting
    /// empty list before closing; retired request and ticket keys stay retired.
    pub fn clear(&mut self) {
        self.active.clear();
        self.dirty |= self.connected;
    }

    pub fn active_count(&self) -> usize {
        self.active.len()
    }

    /// Request a fresh full-list publication, for a bounded service heartbeat.
    pub fn request_refresh(&mut self) {
        self.dirty |= self.connected;
    }

    /// Check before processing inbound callbacks and game packets. Also call
    /// periodically while idle; successful validation never disables expiry.
    pub fn expire(&mut self, now: Instant, unix_now: u64) -> Vec<AuthEvent> {
        let events: Vec<_> = self
            .active
            .iter()
            .filter_map(|(&request_id, session)| {
                let outcome = if unix_now >= session.ticket.expires_at {
                    AuthOutcome::Expired
                } else if !session.accepted && now >= session.deadline {
                    AuthOutcome::TimedOut
                } else {
                    return None;
                };
                Some(AuthEvent {
                    request_id,
                    outcome,
                })
            })
            .collect();
        for event in &events {
            self.end(event.request_id);
        }
        events
    }

    /// Terminal on transport/send failure. All accepted and pending peers lose
    /// authentication; future callbacks on this instance have no authority.
    pub fn disconnected(&mut self) -> Vec<AuthEvent> {
        let events = self
            .active
            .keys()
            .map(|&request_id| AuthEvent {
                request_id,
                outcome: AuthOutcome::Disconnected,
            })
            .collect();
        self.active.clear();
        self.connected = false;
        self.dirty = false;
        events
    }

    /// Return one complete list after a mutation or a CM resend request. A send
    /// failure must call `disconnected`; never silently drop this publication.
    pub fn take_auth_list(&mut self) -> Result<Option<ClientAuthList>> {
        if !self.connected || !self.dirty {
            return Ok(None);
        }
        let previous = self.sequence;
        let sequence = previous
            .checked_add(1)
            .context("Steam authentication sequence exhausted")?;
        let tickets = self
            .active
            .values_mut()
            .map(|session| {
                session.submitted = true;
                AuthTicket {
                    estate: Some(session.estate),
                    steamid: Some(session.ticket.steam_id),
                    gameid: Some(u64::from(self.app_id)),
                    ticket_crc: Some(session.ticket.crc),
                    ticket: Some(session.ticket.prefix.to_vec()),
                    ..AuthTicket::default()
                }
            })
            .collect();
        self.sequence = sequence;
        self.dirty = false;
        Ok(Some(ClientAuthList {
            tokens_left: Some(0),
            last_request_seq: Some(previous),
            last_request_seq_from_server: Some(self.ack_sequence),
            tickets,
            app_ids: vec![self.app_id],
            message_sequence: Some(sequence),
        }))
    }

    /// Dispatch beside presence replies. ACK is only a receipt, never Accepted.
    /// Missing required callback fields fail closed, without credential output.
    /// The returned `estate` is retained for subsequent list submissions.
    /// No relation between callback sequence and session counters is assumed.
    pub fn handle_packet(&mut self, packet: &Packet) -> Result<Vec<AuthEvent>> {
        if !self.connected {
            return Ok(Vec::new());
        }
        if !matches!(
            packet.emsg,
            emsg::CLIENT_AUTH_LIST_ACK
                | emsg::CLIENT_TICKET_AUTH_COMPLETE
                | emsg::CLIENT_REQUEST_AUTH_LIST
        ) {
            return Ok(Vec::new());
        }
        ensure!(
            packet.proto && packet.body.len() <= MAX_CM_AUTH_BYTES,
            "invalid CM authentication envelope"
        );
        match packet.emsg {
            emsg::CLIENT_REQUEST_AUTH_LIST => {
                self.dirty = true;
            }
            emsg::CLIENT_AUTH_LIST_ACK => {
                let ack = ClientAuthListAck::decode(packet.body.as_slice())
                    .map_err(|_| anyhow::anyhow!("invalid CM authentication acknowledgement"))?;
                if let Some(sequence) = ack.message_sequence {
                    self.ack_sequence = sequence;
                }
            }
            emsg::CLIENT_TICKET_AUTH_COMPLETE => {
                let result = ClientTicketAuthComplete::decode(packet.body.as_slice())
                    .map_err(|_| anyhow::anyhow!("invalid CM authentication result"))?;
                let steam_id = result
                    .steam_id
                    .context("CM authentication result lacks identity")?;
                let game_id = result
                    .game_id
                    .context("CM authentication result lacks app")?;
                let crc = result
                    .ticket_crc
                    .context("CM authentication result lacks CRC")?;
                let response = result
                    .eauth_session_response
                    .context("CM authentication result lacks response")?;
                let estate = result
                    .estate
                    .context("CM authentication result lacks state")?;
                if game_id != u64::from(self.app_id) {
                    return Ok(Vec::new());
                }
                let Some((&request_id, session)) = self.active.iter_mut().find(|(_, session)| {
                    session.submitted
                        && session.ticket.steam_id == steam_id
                        && session.ticket.crc == crc
                }) else {
                    return Ok(Vec::new());
                };
                if response != 0 {
                    let outcome = if session.accepted {
                        AuthOutcome::Revoked
                    } else {
                        AuthOutcome::Rejected
                    };
                    self.end(request_id);
                    return Ok(vec![AuthEvent {
                        request_id,
                        outcome,
                    }]);
                }
                session.estate = estate;
                if !session.accepted {
                    session.accepted = true;
                    return Ok(vec![AuthEvent {
                        request_id,
                        outcome: AuthOutcome::Accepted,
                    }]);
                }
            }
            _ => unreachable!("auth message filter"),
        }
        Ok(Vec::new())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steam_fetch::proto::ProtoHeader;

    const USER: u64 = 0x0110_0001_0000_002a;
    const APP: u32 = 1_149_460;

    // Independently generated RSA test key/signature over an authored ownership
    // ticket. Its private key was not retained; this is not Valve-signed data.
    const TEST_MODULUS: &str = "cf7f1ec273c3a53b1f69a1a87ae57c359f456bdb50ad5c6f027879a711d758757ddbe71fc41b6898ebd80322ac0b0f935b5e9131a13812498335f5aa85ec79fe258d6e5e8145f6deeab5c7cf284d6c36b0a66e4ec6c1b6387c7457c0e48c1c464f842c73cad4b57e3457abad38b30b43b0512b993b99aebd2594984708da3333";
    const TEST_SIGNATURE: &str = "6f869a1ba01553119cd2bc83dbacbf62b7bad183ce84d8689618dbb2c2454673978dbcb2c686fd3f2bbccdcc0d00352c8dd1326bf54eb31e67a0165214e37736af4acde27b1119569c0d5d2ee294fb7a6e6fa9d3575d447454bd66b2343d7dd25c70d2bbbb6110d625bdd3a34a296a602fc9e23c4b51f9a8bd6d3606af82bd44";

    fn hex(value: &str) -> Vec<u8> {
        decode_ticket_hex(&value.encode_utf16().collect::<Vec<_>>()).unwrap()
    }

    fn signed_fixture() -> Vec<u8> {
        let mut ticket = vec![0; 56 + 46];
        ticket[0..4].copy_from_slice(&20u32.to_le_bytes());
        ticket[12..20].copy_from_slice(&USER.to_le_bytes());
        ticket[24..28].copy_from_slice(&24u32.to_le_bytes());
        ticket[28..32].copy_from_slice(&1u32.to_le_bytes());
        ticket[32..36].copy_from_slice(&2u32.to_le_bytes());
        ticket[52..56].copy_from_slice(&174u32.to_le_bytes());
        ticket[56..60].copy_from_slice(&46u32.to_le_bytes());
        ticket[60..64].copy_from_slice(&4u32.to_le_bytes());
        ticket[64..72].copy_from_slice(&USER.to_le_bytes());
        ticket[72..76].copy_from_slice(&APP.to_le_bytes());
        ticket[88..92].copy_from_slice(&1000u32.to_le_bytes());
        ticket[92..96].copy_from_slice(&2000u32.to_le_bytes());
        ticket.extend_from_slice(&hex(TEST_SIGNATURE));
        ticket
    }

    fn verify_fixture(ticket: &[u8], now: u64) -> Result<VerifiedTicket> {
        VerifiedTicket::verify_with_key(ticket, USER, APP, now, &hex(TEST_MODULUS), &[1, 0, 1])
    }

    fn ticket() -> VerifiedTicket {
        verify_fixture(&signed_fixture(), 1500).unwrap()
    }

    fn sessions() -> AuthSessions {
        AuthSessions::new(APP, 2, 4, Duration::from_secs(10)).unwrap()
    }

    fn packet(message: &impl prost::Message, emsg: u32) -> Packet {
        Packet {
            emsg,
            proto: true,
            header: ProtoHeader::default(),
            body: message.encode_to_vec(),
        }
    }

    fn completion(response: Option<u32>) -> Packet {
        packet(
            &ClientTicketAuthComplete {
                steam_id: Some(USER),
                game_id: Some(u64::from(APP)),
                estate: Some(2),
                eauth_session_response: response,
                ticket_crc: Some(ticket().crc),
                ..ClientTicketAuthComplete::default()
            },
            emsg::CLIENT_TICKET_AUTH_COMPLETE,
        )
    }

    #[test]
    fn signature_identity_expiry_and_exact_lengths_are_required() {
        let bytes = signed_fixture();
        assert!(verify_fixture(&bytes, 1500).is_ok());
        assert!(VerifiedTicket::verify(&bytes, USER, APP, 1500).is_err());
        assert!(verify_fixture(&bytes, 999).is_err());
        assert!(verify_fixture(&bytes, 2000).is_err());
        assert!(
            VerifiedTicket::verify_with_key(
                &bytes,
                USER + 1,
                APP,
                1500,
                &hex(TEST_MODULUS),
                &[1, 0, 1]
            )
            .is_err()
        );
        assert!(
            VerifiedTicket::verify_with_key(
                &bytes,
                USER,
                APP + 1,
                1500,
                &hex(TEST_MODULUS),
                &[1, 0, 1]
            )
            .is_err()
        );
        for offset in [
            0,
            12,
            24,
            28,
            32,
            52,
            56,
            60,
            64,
            72,
            76,
            88,
            92,
            96,
            98,
            100,
            102,
            bytes.len() - 1,
        ] {
            let mut changed = bytes.clone();
            changed[offset] ^= 1;
            assert!(verify_fixture(&changed, 1500).is_err(), "offset {offset}");
        }
        for end in 0..bytes.len() {
            assert!(verify_fixture(&bytes[..end], 1500).is_err());
        }
        let mut trailing = bytes.clone();
        trailing.push(0);
        assert!(verify_fixture(&trailing, 1500).is_err());
    }

    #[test]
    fn acknowledgement_never_accepts_and_revocation_is_terminal() {
        let mut sessions = sessions();
        sessions.begin(9, ticket(), Instant::now()).unwrap();
        assert!(
            sessions
                .handle_packet(&completion(Some(0)))
                .unwrap()
                .is_empty()
        );
        let list = sessions.take_auth_list().unwrap().unwrap();
        assert_eq!(list.tickets.len(), 1);
        assert_eq!(list.app_ids, [APP]);
        assert_eq!(list.tickets[0].ticket.as_deref().unwrap().len(), 52);
        let ack = packet(
            &ClientAuthListAck {
                ticket_crc: vec![ticket().crc],
                app_ids: vec![APP],
                message_sequence: Some(1),
            },
            emsg::CLIENT_AUTH_LIST_ACK,
        );
        assert!(sessions.handle_packet(&ack).unwrap().is_empty());
        assert!(sessions.handle_packet(&completion(None)).is_err());
        assert_eq!(
            sessions.handle_packet(&completion(Some(0))).unwrap(),
            [AuthEvent {
                request_id: 9,
                outcome: AuthOutcome::Accepted
            }]
        );
        assert!(
            sessions
                .handle_packet(&completion(Some(0)))
                .unwrap()
                .is_empty()
        );
        assert_eq!(
            sessions.handle_packet(&completion(Some(6))).unwrap(),
            [AuthEvent {
                request_id: 9,
                outcome: AuthOutcome::Revoked
            }]
        );
        assert!(
            sessions
                .handle_packet(&completion(Some(0)))
                .unwrap()
                .is_empty()
        );
        let list = sessions.take_auth_list().unwrap().unwrap();
        assert!(list.tickets.is_empty());
        assert_eq!(list.app_ids, [APP]);
        assert_eq!(list.last_request_seq_from_server, Some(1));
        assert!(sessions.begin(10, ticket(), Instant::now()).is_err());
    }

    #[test]
    fn callback_before_ack_and_mismatched_identity_are_handled() {
        let mut sessions = sessions();
        sessions.begin(9, ticket(), Instant::now()).unwrap();
        sessions.take_auth_list().unwrap();
        let correct = completion(Some(0));
        let original = ClientTicketAuthComplete::decode(correct.body.as_slice()).unwrap();
        for result in [
            ClientTicketAuthComplete {
                steam_id: Some(USER + 1),
                ..original.clone()
            },
            ClientTicketAuthComplete {
                game_id: Some(u64::from(APP + 1)),
                ..original.clone()
            },
            ClientTicketAuthComplete {
                ticket_crc: Some(ticket().crc ^ 1),
                ..original.clone()
            },
        ] {
            assert!(
                sessions
                    .handle_packet(&packet(&result, emsg::CLIENT_TICKET_AUTH_COMPLETE))
                    .unwrap()
                    .is_empty()
            );
        }
        assert_eq!(
            sessions.handle_packet(&correct).unwrap()[0].outcome,
            AuthOutcome::Accepted
        );
    }

    #[test]
    fn deadlines_disconnect_and_correlation_reuse_fail_closed() {
        let now = Instant::now();
        let mut sessions = sessions();
        sessions.begin(1, ticket(), now).unwrap();
        sessions.take_auth_list().unwrap();
        assert_eq!(
            sessions.expire(now + Duration::from_secs(10), 1500),
            [AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::TimedOut
            }]
        );
        assert!(
            sessions
                .handle_packet(&completion(Some(0)))
                .unwrap()
                .is_empty()
        );
        let mut changed = ticket();
        changed.crc ^= 1;
        assert!(sessions.begin(1, changed, now).is_err());
        assert!(sessions.begin(2, ticket(), now).is_err());
        let mut changed = ticket();
        changed.crc ^= 1;
        sessions.begin(2, changed, now).unwrap();
        assert_eq!(
            sessions.disconnected(),
            [AuthEvent {
                request_id: 2,
                outcome: AuthOutcome::Disconnected
            }]
        );
        assert!(sessions.disconnected().is_empty());
        assert!(sessions.take_auth_list().unwrap().is_none());
        assert!(
            sessions
                .handle_packet(&completion(Some(0)))
                .unwrap()
                .is_empty()
        );
        assert!(sessions.begin(3, ticket(), now).is_err());
    }

    #[test]
    fn accepted_ticket_still_expires_and_active_crc_collision_rejects() {
        let now = Instant::now();
        let mut sessions = sessions();
        sessions.begin(1, ticket(), now).unwrap();
        let mut other = ticket();
        other.steam_id += 1;
        assert!(sessions.begin(2, other, now).is_err());
        sessions.take_auth_list().unwrap();
        sessions.handle_packet(&completion(Some(0))).unwrap();
        assert!(
            sessions
                .expire(now + Duration::from_secs(11), 1999)
                .is_empty()
        );
        assert_eq!(
            sessions.expire(now + Duration::from_secs(12), 2000),
            [AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Expired
            }]
        );
    }

    #[test]
    fn limits_refresh_and_full_list_removal_remain_bounded() {
        let now = Instant::now();
        let mut sessions = AuthSessions::new(APP, 1, 2, Duration::from_secs(10)).unwrap();
        sessions.begin(1, ticket(), now).unwrap();
        let mut next = ticket();
        next.crc ^= 1;
        assert!(sessions.begin(2, next, now).is_err());
        sessions.take_auth_list().unwrap();
        assert!(sessions.take_auth_list().unwrap().is_none());
        sessions.request_refresh();
        let refresh = sessions.take_auth_list().unwrap().unwrap();
        assert_eq!(refresh.tickets.len(), 1);
        assert_eq!(refresh.message_sequence, Some(2));
        assert!(sessions.end(1));
        let empty = sessions.take_auth_list().unwrap().unwrap();
        assert!(empty.tickets.is_empty());
        assert_eq!(empty.app_ids, [APP]);
        let mut next = ticket();
        next.crc ^= 1;
        sessions.begin(2, next, now).unwrap();
        sessions.end(2);
        let mut next = ticket();
        next.crc ^= 2;
        assert!(sessions.begin(3, next, now).is_err());
        sessions.disconnected();
        sessions.request_refresh();
        assert!(sessions.take_auth_list().unwrap().is_none());
    }

    #[test]
    fn malformed_or_nonprotobuf_callback_cannot_accept() {
        let mut sessions = sessions();
        sessions.begin(1, ticket(), Instant::now()).unwrap();
        sessions.take_auth_list().unwrap();
        let mut result = completion(Some(0));
        result.proto = false;
        assert!(sessions.handle_packet(&result).is_err());
        result.proto = true;
        result.body = vec![0xff];
        assert!(sessions.handle_packet(&result).is_err());
        result.body = vec![0; MAX_CM_AUTH_BYTES + 1];
        assert!(sessions.handle_packet(&result).is_err());
        assert_eq!(sessions.active_count(), 1);
    }

    #[test]
    fn orderly_clear_publishes_empty_list_without_reusing_correlation() {
        let now = Instant::now();
        let mut sessions = sessions();
        sessions.begin(1, ticket(), now).unwrap();
        sessions.take_auth_list().unwrap();
        sessions.clear();
        let empty = sessions.take_auth_list().unwrap().unwrap();
        assert!(empty.tickets.is_empty());
        assert_eq!(empty.app_ids, [APP]);
        assert_eq!(empty.message_sequence, Some(2));
        assert!(
            sessions
                .handle_packet(&completion(Some(0)))
                .unwrap()
                .is_empty()
        );
        assert!(sessions.begin(2, ticket(), now).is_err());
        let mut changed = ticket();
        changed.crc ^= 1;
        assert!(sessions.begin(1, changed, now).is_err());
    }
}
