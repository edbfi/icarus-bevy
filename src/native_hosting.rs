//! Explicit bounded authenticated hosting for the production runner.
use crate::{config::NetworkConfig, experimental::prospect_info::ProspectInfo};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{net::Ipv4Addr, time::Duration};

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct HostingConfig {
    pub query_bind: Ipv4Addr,
    pub query_port: u16,
    pub private_ip: Ipv4Addr,
    pub seconds: u64,
    pub name: String,
    pub map_url: String,
    pub prospect_id: String,
    pub prospect_row: String,
    pub lobby_name: String,
    pub difficulty: u8,
}

impl HostingConfig {
    pub fn advert(
        &self,
        network: &NetworkConfig,
        pin: &crate::experimental::release::ReleasePin,
    ) -> Result<Advert> {
        let advert = Advert {
            server_steam_id: 0,
            name: self.name.clone(),
            map: self.map_url.clone(),
            game_port: network.port,
            query_port: self.query_port,
            max_players: u8::try_from(network.max_players)?,
            players: Vec::new(),
            occupancy: Some(0),
            master_membership: Some(Default::default()),
            version_tag: crate::steam_presence::version_tag(&pin.project_version)?,
            prospect: Some(self.prospect_id.clone()),
            prospect_info: Some(crate::steam_presence::encode_prospect_info(
                &self.prospect(),
            )?),
            password_protected: network.join_password.is_set(),
        };
        advert.validate()?;
        Ok(advert)
    }
    pub fn validate(&self, network: &NetworkConfig) -> Result<()> {
        ensure!(network.enabled, "hosting requires enabled networking");
        ensure!(
            network.port != 0 && self.query_port != 0,
            "hosting requires explicit nonzero ports"
        );
        ensure!(
            !(network.port == self.query_port
                && (network.bind == self.query_bind
                    || network.bind.is_unspecified()
                    || self.query_bind.is_unspecified())),
            "hosting listeners overlap"
        );
        ensure!(
            !network.bind.is_multicast()
                && !network.bind.is_broadcast()
                && !self.query_bind.is_multicast()
                && !self.query_bind.is_broadcast(),
            "invalid hosting bind address"
        );
        ensure!(
            (1..=86_400).contains(&self.seconds),
            "hosting seconds must be 1..=86400"
        );
        ensure!(
            network.release_profile.is_some() && network.authored_world.is_some(),
            "hosting requires a pinned release and fresh authored world"
        );
        ensure!(
            network.data_pak.is_some() && network.authored_gameplay.is_some(),
            "hosting requires protocol catalog and authored gameplay"
        );
        ensure!(
            self.map_url.len() <= 512
                && !self.map_url.contains('?')
                && !self.map_url.chars().any(char::is_control),
            "invalid authored hosting map"
        );
        crate::experimental::session_open::map_level_chain(&self.map_url)?;
        for value in [
            &self.prospect_id,
            &self.prospect_row,
            &self.lobby_name,
            &self.name,
        ] {
            ensure!(
                !value.is_empty()
                    && value.len() <= 63
                    && !value
                        .chars()
                        .any(|c| c.is_control() || c == ',' || c == ':'),
                "invalid hosting identity"
            );
        }
        self.prospect().validate_enumerators()?;
        crate::steam_presence::encode_prospect_info(&self.prospect())?;
        crate::steam_service::ServiceLimits::default().validate()?;
        Ok(())
    }

    pub fn duration(&self) -> Duration {
        Duration::from_secs(self.seconds)
    }

    pub fn prospect(&self) -> ProspectInfo {
        ProspectInfo {
            prospect_id: self.prospect_id.encode_utf16().collect(),
            prospect_dt_key: self.prospect_row.encode_utf16().collect(),
            lobby_name: self.lobby_name.encode_utf16().collect(),
            expire_time: -1,
            prospect_state: 2,
            difficulty: self.difficulty,
            ..ProspectInfo::default()
        }
    }
}

use crate::{
    steam_fetch::net::{self, HttpTimeouts, Tls},
    steam_presence::{
        Advert, PresenceHandles, QueryResponder, RegistrationTiming, logon_game_server,
        serve_prepared_queries,
    },
    steam_service::{AuthClient, ServiceLimits, SteamService},
};
use anyhow::Context;
use std::{
    net::{SocketAddrV4, UdpSocket},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread::{self, JoinHandle},
    time::Instant,
};

/// Owns every hosting worker. Drop covers partial startup as well as exits.
/// A query worker is started only when the completed application is activated.
pub struct NativeHosting {
    presence: PresenceHandles,
    service: Option<SteamService>,
    query: Option<UdpSocket>,
    worker: Option<JoinHandle<Result<u64>>>,
    deadline: Instant,
    responder: Option<QueryResponder>,
}

impl NativeHosting {
    pub fn start(config: &HostingConfig, mut advert: Advert) -> Result<Self> {
        advert.validate()?;
        let query = UdpSocket::bind(SocketAddrV4::new(config.query_bind, config.query_port))
            .context("bind hosting query listener")?;
        query.set_read_timeout(Some(Duration::from_millis(250)))?;
        let responder = QueryResponder::new()?;
        let tls = Tls::from_native_roots()?;
        let http = net::http_agent(&tls, &HttpTimeouts::default());
        let (session, logon, _) = logon_game_server(
            &tls,
            &http,
            config.private_ip,
            RegistrationTiming::default(),
        )?;
        advert.server_steam_id = logon.steam_id;
        let presence = PresenceHandles {
            advert: Arc::new(Mutex::new(advert)),
            stop: Arc::new(AtomicBool::new(false)),
        };
        let deadline = Instant::now() + config.duration();
        let service = SteamService::spawn_unlisted(
            session,
            presence.clone(),
            deadline,
            ServiceLimits::default(),
        )?;
        Ok(Self {
            presence,
            service: Some(service),
            query: Some(query),
            worker: None,
            deadline,
            responder: Some(responder),
        })
    }

    pub fn client(&self) -> AuthClient {
        self.service
            .as_ref()
            .expect("owned hosting service")
            .client()
    }

    pub fn activate(&mut self) -> Result<()> {
        ensure!(!self.stopped(), "hosting backend stopped before activation");
        let query = self.query.take().context("hosting already activated")?;
        let presence = self.presence.clone();
        let responder = self.responder.take().context("query responder missing")?;
        self.worker = Some(spawn_query(query, presence, responder)?);
        self.service
            .as_ref()
            .context("hosting service missing")?
            .activate();
        Ok(())
    }

    pub fn stopped(&self) -> bool {
        self.presence.stop.load(Ordering::Acquire)
            || Instant::now() >= self.deadline
            || self.worker.as_ref().is_some_and(JoinHandle::is_finished)
    }

    pub fn membership(&self, membership: crate::steam_presence::MasterMembership) -> Result<()> {
        let mut advert = self
            .presence
            .advert
            .lock()
            .map_err(|_| anyhow::anyhow!("hosting advert unavailable"))?;
        let joined = u8::try_from(membership.len()).context("hosting occupancy overflow")?;
        ensure!(
            joined <= advert.max_players,
            "hosting occupancy exceeds capacity"
        );
        advert.occupancy = Some(joined);
        advert.master_membership = Some(membership);
        Ok(())
    }

    pub fn stop_and_join(mut self) -> Result<()> {
        self.cleanup()
    }

    fn cleanup(&mut self) -> Result<()> {
        self.presence.stop.store(true, Ordering::Release);
        if let Ok(mut advert) = self.presence.advert.lock() {
            advert.master_membership = Some(Default::default());
            advert.occupancy = Some(0);
        }
        self.query.take();
        let query_result = self
            .worker
            .take()
            .map(|worker| {
                worker
                    .join()
                    .map_err(|_| anyhow::anyhow!("hosting query worker failed"))
                    .and_then(|result| result)
            })
            .transpose();
        let steam_result = self
            .service
            .take()
            .map(SteamService::stop_and_join)
            .transpose();
        query_result?;
        steam_result?;
        Ok(())
    }
}

impl Drop for NativeHosting {
    fn drop(&mut self) {
        let _ = self.cleanup();
    }
}

fn spawn_query(
    query: UdpSocket,
    presence: PresenceHandles,
    responder: QueryResponder,
) -> Result<JoinHandle<Result<u64>>> {
    thread::Builder::new()
        .name("hosting-query".into())
        .spawn(move || {
            struct Stop(Arc<AtomicBool>);
            impl Drop for Stop {
                fn drop(&mut self) {
                    self.0.store(true, Ordering::Release);
                }
            }
            let _stop = Stop(presence.stop.clone());
            serve_prepared_queries(&query, &presence.advert, &presence.stop, responder)
        })
        .context("spawn hosting query worker")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authored_world_bootstrap::AuthoredWorldBootstrap;

    fn config() -> (NetworkConfig, HostingConfig) {
        let hosting = HostingConfig {
            query_bind: Ipv4Addr::LOCALHOST,
            query_port: 27015,
            private_ip: Ipv4Addr::LOCALHOST,
            seconds: 60,
            name: "Authored host".into(),
            map_url: "/Authored/Maps/Workshop".into(),
            prospect_id: "stable-authored-session".into(),
            prospect_row: "AuthoredProspect".into(),
            lobby_name: "Workshop".into(),
            difficulty: 2,
        };
        let network = NetworkConfig {
            enabled: true,
            release_profile: Some("authored-release.toml".into()),
            data_pak: Some("authored-catalog.pak".into()),
            authored_gameplay: Some("authored-gameplay.json".into()),
            authored_world: Some(AuthoredWorldBootstrap {
                seed: 42,
                seed_initialised: true,
                flod_tile_count: 0,
                clock: None,
            }),
            ..NetworkConfig::default()
        };
        (network, hosting)
    }

    #[test]
    fn opt_in_bounds_and_shared_identity() {
        let (mut network, mut hosting) = config();
        assert!(NetworkConfig::default().hosting.is_none());
        network.bind = Ipv4Addr::UNSPECIFIED;
        assert!(network.validate().is_err());
        network.hosting = Some(hosting.clone());
        network.validate().unwrap();
        let profile = crate::experimental::release::ReleaseProfile::parse(include_str!(
            "../config/release.toml"
        ))
        .unwrap();
        let advert = hosting.advert(&network, &profile.pin).unwrap();
        assert_eq!(advert.map, hosting.map_url);
        assert_eq!(
            advert.prospect.as_deref(),
            Some(hosting.prospect_id.as_str())
        );
        assert_eq!(
            advert.prospect_info,
            Some(crate::steam_presence::encode_prospect_info(&hosting.prospect()).unwrap())
        );
        assert_eq!(advert.game_port, network.port);
        assert_eq!(advert.query_port, hosting.query_port);
        for seconds in [0, 86_401, u64::MAX] {
            hosting.seconds = seconds;
            assert!(hosting.validate(&network).is_err());
        }
        hosting.seconds = 86_400;
        hosting.validate(&network).unwrap();
        network.enabled = false;
        assert!(hosting.validate(&network).is_err());
    }

    #[test]
    fn hosting_serialization_omits_join_secret() {
        let (mut network, hosting) = config();
        network.hosting = Some(hosting);
        network.join_password =
            crate::experimental::admission::JoinPassword::new("authored-secret").unwrap();
        let serialized = toml::to_string(&network).unwrap();
        assert!(!serialized.contains("authored-secret"));
        assert!(!format!("{network:?}").contains("authored-secret"));
    }

    fn query_only_owner() -> (NativeHosting, std::net::SocketAddr) {
        let (network, config) = config();
        let profile = crate::experimental::release::ReleaseProfile::parse(include_str!(
            "../config/release.toml"
        ))
        .unwrap();
        let advert = config.advert(&network, &profile.pin).unwrap();
        let query = UdpSocket::bind("127.0.0.1:0").unwrap();
        query
            .set_read_timeout(Some(Duration::from_millis(20)))
            .unwrap();
        let address = query.local_addr().unwrap();
        (
            NativeHosting {
                presence: PresenceHandles {
                    advert: Arc::new(Mutex::new(advert)),
                    stop: Arc::new(AtomicBool::new(false)),
                },
                service: None,
                query: Some(query),
                worker: None,
                deadline: Instant::now() + Duration::from_secs(1),
                responder: Some(QueryResponder::with_secret([7; 32], Instant::now())),
            },
            address,
        )
    }

    #[test]
    fn partial_startup_drop_releases_query_socket() {
        let (owner, address) = query_only_owner();
        assert!(UdpSocket::bind(address).is_err());
        drop(owner);
        UdpSocket::bind(address).unwrap();
    }

    #[test]
    fn query_worker_stop_joins_and_releases_listener() {
        let (mut owner, address) = query_only_owner();
        owner.worker = Some(
            spawn_query(
                owner.query.take().unwrap(),
                owner.presence.clone(),
                owner.responder.take().unwrap(),
            )
            .unwrap(),
        );
        owner.stop_and_join().unwrap();
        UdpSocket::bind(address).unwrap();
    }

    #[test]
    fn partial_activation_failure_still_joins_query_worker() {
        let (mut owner, address) = query_only_owner();
        // No Steam service exists in this authored fixture. Activation fails
        // after taking ownership of the query worker; Drop must join it.
        assert!(owner.activate().is_err());
        drop(owner);
        UdpSocket::bind(address).unwrap();
    }

    #[test]
    fn query_error_sets_shared_terminal_stop() {
        use crate::experimental::server_query::{Request, encode_request};
        let (mut owner, address) = query_only_owner();
        let poisoned = owner.presence.advert.clone();
        assert!(
            thread::spawn(move || {
                let _guard = poisoned.lock().unwrap();
                panic!("authored mutex failure");
            })
            .join()
            .is_err()
        );
        owner.worker = Some(
            spawn_query(
                owner.query.take().unwrap(),
                owner.presence.clone(),
                owner.responder.take().unwrap(),
            )
            .unwrap(),
        );
        let client = UdpSocket::bind("127.0.0.1:0").unwrap();
        client
            .set_read_timeout(Some(Duration::from_secs(1)))
            .unwrap();
        client
            .send_to(&encode_request(&Request::Rules { challenge: -1 }), address)
            .unwrap();
        let mut bytes = [0; 128];
        let (length, _) = client.recv_from(&mut bytes).unwrap();
        let crate::experimental::server_query::Response::Challenge(challenge) =
            crate::experimental::server_query::parse_response(&bytes[..length]).unwrap()
        else {
            panic!("expected challenge");
        };
        client
            .send_to(&encode_request(&Request::Rules { challenge }), address)
            .unwrap();
        // The one worker consumes the verified query, observes the poisoned
        // advert, and publishes stop through its exit guard.
        let deadline = Instant::now() + Duration::from_secs(1);
        while !owner.worker.as_ref().unwrap().is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
        let stopped_by_worker = owner.presence.stop.load(Ordering::Acquire);
        owner.presence.stop.store(true, Ordering::Release);
        let result = owner.worker.take().unwrap().join().unwrap();
        assert!(stopped_by_worker);
        assert!(result.is_err());
        assert!(owner.stopped());
        drop(owner);
        UdpSocket::bind(address).unwrap();
    }

    #[test]
    fn stop_and_deadline_are_terminal_and_counts_are_bounded() {
        let (mut owner, address) = query_only_owner();
        assert!(!owner.stopped());
        owner
            .membership(
                crate::steam_presence::MasterMembership::from_authenticated([11, 22, 33]).unwrap(),
            )
            .unwrap();
        assert_eq!(owner.presence.advert.lock().unwrap().occupancy, Some(3));
        {
            let advert = owner.presence.advert.lock().unwrap();
            advert.validate().unwrap();
            assert_eq!(advert.game_server_data(1).players.len(), 3);
            assert!(advert.players().is_empty());
            assert!(!format!("{advert:?}").contains("steam_id: Some(11)"));
        }
        assert!(
            owner
                .membership(
                    crate::steam_presence::MasterMembership::from_authenticated(1..=9).unwrap()
                )
                .is_err()
        );
        assert_eq!(owner.presence.advert.lock().unwrap().occupancy, Some(3));
        owner.membership(Default::default()).unwrap();
        {
            let advert = owner.presence.advert.lock().unwrap();
            advert.validate().unwrap();
            assert_eq!(advert.info().players, 0);
            assert!(advert.game_server_data(2).players.is_empty());
        }
        owner.deadline = Instant::now();
        assert!(owner.stopped());
        owner.stop_and_join().unwrap();
        UdpSocket::bind(address).unwrap();
    }
}
