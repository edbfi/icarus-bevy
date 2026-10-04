//! Loopback-only Bevy lab for the recovered short stateless handshake.
//!
//! This does not create connections, authenticate players or implement ICARUS
//! gameplay. Its HMAC policy is independently designed, not the original algorithm.
use std::{
    io,
    net::{Ipv4Addr, SocketAddr, SocketAddrV4, UdpSocket},
    time::Instant,
};

use bevy::prelude::*;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use super::handshake::{LegacyHandshake, decode, encode, initial_request};

/// Bound the work of one Bevy update, including malformed datagrams.
pub const PACKETS_PER_UPDATE: usize = 32;
const COOKIE_LIFETIME_SECONDS: f64 = 40.0;

struct CookieAuthority([u8; 32]);

impl CookieAuthority {
    fn mac(&self, peer: SocketAddrV4, timestamp: f64) -> Hmac<Sha256> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.0).expect("fixed HMAC key size");
        mac.update(b"icarus-bevy/handshake-lab/v1\0");
        mac.update(&peer.ip().octets());
        mac.update(&peer.port().to_be_bytes());
        mac.update(&timestamp.to_le_bytes());
        mac
    }

    fn reply(&self, peer: SocketAddrV4, bytes: &[u8], now: f64) -> Option<[u8; 29]> {
        if !now.is_finite() || now <= 0.0 {
            return None;
        }
        if bytes == initial_request() {
            let cookie = self.mac(peer, now).finalize().into_bytes()[..20]
                .try_into()
                .expect("20-byte prefix");
            return encode(&LegacyHandshake {
                restart: false,
                secret_id: false,
                timestamp: now,
                cookie,
            })
            .ok();
        }
        let packet = decode(bytes).ok()?;
        if packet.restart
            || packet.secret_id
            || packet.timestamp <= 0.0
            || packet.timestamp > now
            || now - packet.timestamp >= COOKIE_LIFETIME_SECONDS
        {
            return None;
        }
        self.mac(peer, packet.timestamp)
            .verify_truncated_left(&packet.cookie)
            .ok()?;
        encode(&LegacyHandshake {
            restart: false,
            secret_id: true,
            timestamp: -1.0,
            cookie: packet.cookie,
        })
        .ok()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct LabStats {
    pub received: u64,
    pub rejected: u64,
    pub challenges_sent: u64,
    pub acknowledgements_sent: u64,
    pub io_errors: u64,
}

#[derive(Resource)]
pub struct HandshakeLab {
    socket: UdpSocket,
    authority: CookieAuthority,
    started: Instant,
    stats: LabStats,
}

impl HandshakeLab {
    /// Bind IPv4 loopback only; zero lets the OS select an ephemeral port.
    pub fn bind(port: u16) -> io::Result<Self> {
        let mut secret = [0; 32];
        getrandom::fill(&mut secret).map_err(io::Error::other)?;
        let socket = UdpSocket::bind((Ipv4Addr::LOCALHOST, port))?;
        socket.set_nonblocking(true)?;
        Ok(Self {
            socket,
            authority: CookieAuthority(secret),
            started: Instant::now(),
            stats: LabStats::default(),
        })
    }

    pub fn local_addr(&self) -> io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn stats(&self) -> LabStats {
        self.stats
    }
}

/// Insert a bound `HandshakeLab` before adding this optional research plugin.
pub struct HandshakeLabPlugin;

impl Plugin for HandshakeLabPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, poll_lab);
    }
}

fn poll_lab(mut lab: ResMut<HandshakeLab>) {
    // One excess byte distinguishes an exact short frame from a truncated larger
    // UDP datagram. No peer table, application queue or per-peer allocation.
    let mut buffer = [0; 30];
    for _ in 0..PACKETS_PER_UPDATE {
        let (length, peer) = match lab.socket.recv_from(&mut buffer) {
            Ok(received) => received,
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => break,
            Err(_) => {
                lab.stats.io_errors = lab.stats.io_errors.saturating_add(1);
                break;
            }
        };
        lab.stats.received = lab.stats.received.saturating_add(1);
        let SocketAddr::V4(peer) = peer else {
            lab.stats.rejected = lab.stats.rejected.saturating_add(1);
            continue;
        };
        // Start positive, so a challenge cannot be confused with the initial form.
        let now = 1.0 + lab.started.elapsed().as_secs_f64();
        let Some(response) = lab.authority.reply(peer, &buffer[..length], now) else {
            lab.stats.rejected = lab.stats.rejected.saturating_add(1);
            continue;
        };
        match lab.socket.send_to(&response, peer) {
            Ok(29) => {
                if buffer[..length] == initial_request() {
                    lab.stats.challenges_sent = lab.stats.challenges_sent.saturating_add(1);
                } else {
                    lab.stats.acknowledgements_sent =
                        lab.stats.acknowledgements_sent.saturating_add(1);
                }
            }
            _ => lab.stats.io_errors = lab.stats.io_errors.saturating_add(1),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cookie_matches_independent_hmac_oracle() {
        let authority = CookieAuthority([7; 32]);
        let peer = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 12345);
        let challenge = authority.reply(peer, &initial_request(), 123.5).unwrap();
        let parsed = decode(&challenge).unwrap();
        // Generated independently with Python hashlib/hmac, recorded in spec.
        assert_eq!(
            parsed.cookie,
            [
                0xd3, 0x89, 0x07, 0xb0, 0x82, 0x7a, 0xb1, 0x4d, 0xe4, 0x52, 0x7f, 0xa3, 0x4d, 0x89,
                0xe6, 0xdd, 0x3d, 0x8c, 0x80, 0x36
            ]
        );
        let ack = authority.reply(peer, &challenge, 124.0).unwrap();
        let ack = decode(&ack).unwrap();
        assert_eq!(ack.timestamp, -1.0);
        assert!(ack.secret_id);
        assert_eq!(ack.cookie, parsed.cookie);
    }

    #[test]
    fn forged_expired_future_and_wrong_peer_cookies_are_rejected() {
        let authority = CookieAuthority([7; 32]);
        let peer = SocketAddrV4::new(Ipv4Addr::LOCALHOST, 12345);
        let challenge = authority.reply(peer, &initial_request(), 100.0).unwrap();
        for now in [99.0, 140.0, f64::NAN, f64::INFINITY] {
            assert!(authority.reply(peer, &challenge, now).is_none());
        }
        assert!(authority.reply(peer, &challenge, 139.999).is_some());
        for other in [
            SocketAddrV4::new(Ipv4Addr::LOCALHOST, 12346),
            SocketAddrV4::new(Ipv4Addr::new(127, 0, 0, 2), 12345),
        ] {
            assert!(authority.reply(other, &challenge, 101.0).is_none());
        }
        assert!(
            CookieAuthority([8; 32])
                .reply(peer, &challenge, 101.0)
                .is_none()
        );
        for bit in 1..227 {
            let mut altered = challenge;
            altered[bit / 8] ^= 1 << (bit % 8);
            assert!(
                authority.reply(peer, &altered, 101.0).is_none(),
                "bit {bit}"
            );
        }
        let mut noncanonical_initial = initial_request();
        noncanonical_initial[8] |= 4; // Timestamp sign bit: negative zero.
        assert!(
            authority
                .reply(peer, &noncanonical_initial, 101.0)
                .is_none()
        );
    }
}
