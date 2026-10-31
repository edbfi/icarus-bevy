//! One bounded CM owner for native server presence and ticket authentication.
//! The simulation only exchanges bounded commands/events. Any lost queue item,
//! worker exit or CM error makes every client fail closed; stale success events
//! are never returned after that terminal flag is set.
use std::{
    collections::VecDeque,
    fmt,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use anyhow::{Context, Result, ensure};
use prost::Message as _;

use crate::{
    steam_auth::{AuthEvent, AuthOutcome, AuthSessions, VerifiedTicket},
    steam_fetch::{cm::CmSession, frame::emsg, proto::GsStatusReply},
    steam_presence::{Advert, ICARUS_APP_ID, PresenceHandles, RegistrationReport},
};

enum Command {
    Begin(u64, VerifiedTicket),
    End(u64),
}

struct Queues {
    commands: VecDeque<Command>,
    events: VecDeque<AuthEvent>,
}

struct Shared {
    queues: Mutex<Queues>,
    capacity: usize,
    fatal: AtomicBool,
    next_request: AtomicU64,
}

impl Shared {
    fn check(&self) -> Result<()> {
        ensure!(
            !self.fatal.load(Ordering::Acquire),
            "Steam service is unavailable"
        );
        Ok(())
    }

    fn fail(&self) {
        self.fatal.store(true, Ordering::Release);
    }

    fn lock(&self) -> Result<std::sync::MutexGuard<'_, Queues>> {
        self.queues.lock().map_err(|_| {
            self.fail();
            anyhow::anyhow!("Steam service queue is unavailable")
        })
    }
}

/// Simulation-side handle. No CM socket, credentials or approval injection API.
#[derive(Clone)]
pub struct AuthClient {
    shared: Arc<Shared>,
}

impl fmt::Debug for AuthClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("AuthClient")
    }
}

impl AuthClient {
    /// Allocate a unique correlation across every clone and replacement peer lab.
    /// Exhaustion is terminal; IDs never wrap or restart within one service.
    pub fn allocate_request_id(&self) -> Result<u64> {
        self.shared.check()?;
        let id = self
            .shared
            .next_request
            .try_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
            .map_err(|_| {
                self.shared.fail();
                anyhow::anyhow!("Steam service request IDs exhausted")
            })?;
        self.shared.check()?;
        Ok(id)
    }

    fn enqueue(&self, command: Command) -> Result<()> {
        self.shared.check()?;
        let mut queues = self.shared.lock()?;
        self.shared.check()?;
        if queues.commands.len() >= self.shared.capacity {
            self.shared.fail();
            anyhow::bail!("Steam service command capacity exhausted");
        }
        queues.commands.push_back(command);
        Ok(())
    }

    pub fn begin(&self, request_id: u64, ticket: VerifiedTicket) -> Result<()> {
        self.enqueue(Command::Begin(request_id, ticket))
    }

    /// Queue removal exactly once per peer. Failure is terminal, including when
    /// a full queue would otherwise silently retain an authenticated session.
    pub fn end(&self, request_id: u64) -> Result<()> {
        self.enqueue(Command::End(request_id))
    }

    /// A terminal failure overrides queued successes. The caller must revoke all
    /// sessions and stop admission when this returns an error.
    pub fn drain_events(&self) -> Result<Vec<AuthEvent>> {
        self.shared.check()?;
        let mut queues = self.shared.lock()?;
        self.shared.check()?;
        let events = queues.events.drain(..).collect();
        self.shared.check()?;
        Ok(events)
    }

    #[cfg(test)]
    pub(crate) fn test_bridge(capacity: usize) -> (Self, TestBackend) {
        let (client, backend) = bridge(capacity).unwrap();
        (client, TestBackend(backend))
    }
}

struct Backend {
    shared: Arc<Shared>,
}

impl Backend {
    fn commands(&self) -> Result<Vec<Command>> {
        self.shared.check()?;
        let mut queues = self.shared.lock()?;
        self.shared.check()?;
        Ok(queues.commands.drain(..).collect())
    }

    fn event(&self, event: AuthEvent) -> Result<()> {
        self.shared.check()?;
        let mut queues = self.shared.lock()?;
        self.shared.check()?;
        if queues.events.len() >= self.shared.capacity {
            self.shared.fail();
            anyhow::bail!("Steam service event capacity exhausted");
        }
        queues.events.push_back(event);
        Ok(())
    }

    fn events(&self, events: Vec<AuthEvent>) -> Result<()> {
        for event in events {
            self.event(event)?;
        }
        Ok(())
    }
}

impl Drop for Backend {
    fn drop(&mut self) {
        self.shared.fail();
        if let Ok(mut queues) = self.shared.queues.lock() {
            queues.commands.clear();
            queues.events.clear();
        }
    }
}

fn bridge(capacity: usize) -> Result<(AuthClient, Backend)> {
    ensure!(
        (1..=4096).contains(&capacity),
        "invalid Steam service queue capacity"
    );
    let shared = Arc::new(Shared {
        queues: Mutex::new(Queues {
            commands: VecDeque::new(),
            events: VecDeque::new(),
        }),
        capacity,
        fatal: AtomicBool::new(false),
        next_request: AtomicU64::new(1),
    });
    Ok((
        AuthClient {
            shared: shared.clone(),
        },
        Backend { shared },
    ))
}

/// Authored limits for the service's work and queue memory.
#[derive(Clone, Copy, Debug)]
pub struct ServiceLimits {
    pub queue_capacity: usize,
    pub max_active: usize,
    pub max_requests: usize,
    pub auth_timeout: Duration,
    pub poll_interval: Duration,
    pub presence_refresh: Duration,
    pub max_packets_per_poll: usize,
}

impl Default for ServiceLimits {
    fn default() -> Self {
        Self {
            queue_capacity: 128,
            max_active: 64,
            max_requests: 4096,
            auth_timeout: Duration::from_secs(30),
            poll_interval: Duration::from_millis(100),
            presence_refresh: Duration::from_secs(60),
            max_packets_per_poll: 128,
        }
    }
}

impl ServiceLimits {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=4096).contains(&self.queue_capacity),
            "invalid Steam service queue capacity"
        );
        ensure!(
            (Duration::from_millis(1)..=Duration::from_secs(1)).contains(&self.poll_interval),
            "invalid Steam service poll interval"
        );
        ensure!(
            (Duration::from_secs(1)..=Duration::from_secs(300)).contains(&self.presence_refresh),
            "invalid Steam service refresh interval"
        );
        ensure!(
            (1..=1024).contains(&self.max_packets_per_poll),
            "invalid Steam service packet limit"
        );
        AuthSessions::new(
            ICARUS_APP_ID,
            self.max_active,
            self.max_requests,
            self.auth_timeout,
        )?;
        Ok(())
    }
}

/// Owned worker. Dropping it signals stop, revokes clients and joins the thread.
/// The caller logs on synchronously first; this worker is the session's only
/// reader thereafter. Shutdown may wait for the CM's bounded socket I/O.
pub struct SteamService {
    client: AuthClient,
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<Result<RegistrationReport>>>,
}

struct StopOnExit(Arc<AtomicBool>);

impl Drop for StopOnExit {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Release);
    }
}

impl SteamService {
    pub fn spawn(
        session: CmSession,
        presence: PresenceHandles,
        deadline: Instant,
        limits: ServiceLimits,
    ) -> Result<Self> {
        let prepare = (|| {
            limits.validate()?;
            let now = Instant::now();
            ensure!(
                deadline > now && deadline.duration_since(now) <= Duration::from_secs(86_400),
                "invalid Steam service deadline"
            );
            snapshot(&presence.advert)?.validate()?;
            bridge(limits.queue_capacity)
        })();
        let (client, backend) = match prepare {
            Ok(bridge) => bridge,
            Err(error) => {
                session.close_bounded(Duration::from_secs(1));
                return Err(error);
            }
        };
        let stop = presence.stop.clone();
        let worker = thread::Builder::new()
            .name("steam-service".to_owned())
            .spawn(move || {
                let _stop_on_exit = StopOnExit(presence.stop.clone());
                let mut session = session;
                let result = run(&mut session, &presence, deadline, limits, &backend);
                backend.shared.fail();
                presence.stop.store(true, Ordering::Release);
                session.close_bounded(limits.poll_interval);
                result
            })
            .context("spawn Steam service")?;
        Ok(Self {
            client,
            stop,
            worker: Some(worker),
        })
    }

    pub fn client(&self) -> AuthClient {
        self.client.clone()
    }

    pub fn stop_and_join(mut self) -> Result<RegistrationReport> {
        self.stop.store(true, Ordering::Release);
        self.client.shared.fail();
        self.worker
            .take()
            .context("Steam service already joined")?
            .join()
            .map_err(|_| anyhow::anyhow!("Steam service worker failed"))?
    }
}

impl Drop for SteamService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        self.client.shared.fail();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn snapshot(advert: &Mutex<Advert>) -> Result<Advert> {
    Ok(advert
        .lock()
        .map_err(|_| anyhow::anyhow!("Steam advert is unavailable"))?
        .clone())
}

fn unix_now() -> Result<u64> {
    Ok(SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("Steam clock predates Unix epoch")?
        .as_secs())
}

fn run(
    session: &mut CmSession,
    presence: &PresenceHandles,
    deadline: Instant,
    limits: ServiceLimits,
    backend: &Backend,
) -> Result<RegistrationReport> {
    let mut auth = AuthSessions::new(
        ICARUS_APP_ID,
        limits.max_active,
        limits.max_requests,
        limits.auth_timeout,
    )?;
    let first = snapshot(&presence.advert)?;
    first.validate()?;
    let mut revision = 1_u32;
    let mut listed = first.game_server_data(0);
    let mut report = RegistrationReport {
        steam_id: first.server_steam_id,
        ..RegistrationReport::default()
    };
    let outcome = (|| -> Result<()> {
        if presence.stop.load(Ordering::Acquire) {
            return Ok(());
        }
        session.send_bounded(
            emsg::GS_SERVER_TYPE,
            &first.server_type().encode_to_vec(),
            limits.poll_interval,
        )?;
        session.send_bounded(
            emsg::AM_GAME_SERVER_UPDATE,
            &first.game_server_data(revision).encode_to_vec(),
            limits.poll_interval,
        )?;
        report.updates_sent = 1;
        let mut last_update = Instant::now();
        let mut last_auth_update = Instant::now();
        auth.request_refresh();
        while !presence.stop.load(Ordering::Acquire) && Instant::now() < deadline {
            backend.shared.check()?;
            for command in backend.commands()? {
                match command {
                    Command::Begin(id, ticket) => {
                        if auth.begin(id, ticket, Instant::now()).is_err() {
                            backend.event(AuthEvent {
                                request_id: id,
                                outcome: AuthOutcome::Rejected,
                            })?;
                        }
                    }
                    Command::End(id) => {
                        auth.end(id);
                    }
                }
            }
            backend.events(auth.expire(Instant::now(), unix_now()?))?;
            if last_auth_update.elapsed() >= limits.presence_refresh {
                auth.request_refresh();
            }
            if let Some(list) = auth.take_auth_list()? {
                session.send_bounded(
                    emsg::CLIENT_AUTH_LIST,
                    &list.encode_to_vec(),
                    limits.poll_interval,
                )?;
                last_auth_update = Instant::now();
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                break;
            }
            let packets = session.poll_bounded(
                limits.poll_interval.min(remaining),
                limits.max_packets_per_poll,
            )?;
            if presence.stop.load(Ordering::Acquire) || Instant::now() >= deadline {
                break;
            }
            // Expiry wins over an acceptance already queued on the socket.
            backend.events(auth.expire(Instant::now(), unix_now()?))?;
            for packet in packets {
                backend.events(auth.handle_packet(&packet)?)?;
                if packet.emsg == emsg::GS_STATUS_REPLY {
                    let reply = GsStatusReply::decode(packet.body.as_slice())
                        .map_err(|_| anyhow::anyhow!("invalid Steam server status reply"))?;
                    report.status_replies = report.status_replies.saturating_add(1);
                    report.secure = reply.is_secure.or(report.secure);
                }
            }
            let current = snapshot(&presence.advert)?;
            current.validate()?;
            ensure!(
                current.server_steam_id == first.server_steam_id,
                "Steam advert identity changed during service"
            );
            let data = current.game_server_data(0);
            if data != listed || last_update.elapsed() >= limits.presence_refresh {
                revision = revision
                    .checked_add(1)
                    .context("Steam advert revision exhausted")?;
                session.send_bounded(
                    emsg::AM_GAME_SERVER_UPDATE,
                    &current.game_server_data(revision).encode_to_vec(),
                    limits.poll_interval,
                )?;
                report.updates_sent = report.updates_sent.saturating_add(1);
                listed = data;
                last_update = Instant::now();
            }
        }
        Ok(())
    })();
    if let Err(error) = outcome
        && !presence.stop.load(Ordering::Acquire)
    {
        auth.disconnected();
        return Err(error);
    }
    auth.clear();
    if let Some(list) = auth.take_auth_list()? {
        // Normal stop withdraws all tickets. On a failed write the caller still
        // closes the CM, which revokes the service; no retry on this connection.
        session.send_bounded(
            emsg::CLIENT_AUTH_LIST,
            &list.encode_to_vec(),
            limits.poll_interval,
        )?;
    }
    auth.disconnected();
    Ok(report)
}

#[cfg(test)]
pub(crate) struct TestBackend(Backend);

#[cfg(test)]
impl TestBackend {
    pub(crate) fn ended_requests(&self) -> Vec<u64> {
        self.0
            .commands()
            .unwrap()
            .into_iter()
            .filter_map(|command| match command {
                Command::End(id) => Some(id),
                Command::Begin(..) => None,
            })
            .collect()
    }
    pub(crate) fn event(&self, event: AuthEvent) -> Result<()> {
        self.0.event(event)
    }
    pub(crate) fn fail(&self) {
        self.0.shared.fail();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::steam_auth::AuthOutcome;

    #[test]
    fn command_overflow_is_terminal_even_for_end() {
        let (client, backend) = AuthClient::test_bridge(1);
        client.end(1).unwrap();
        assert!(client.end(2).is_err());
        assert!(client.drain_events().is_err());
        assert!(backend.0.commands().is_err());
    }

    #[test]
    fn end_is_delivered_and_clones_share_the_queue() {
        let (client, backend) = AuthClient::test_bridge(2);
        client.end(17).unwrap();
        client.clone().end(18).unwrap();
        let commands = backend.0.commands().unwrap();
        assert!(matches!(
            commands.as_slice(),
            [Command::End(17), Command::End(18)]
        ));
        assert!(backend.0.commands().unwrap().is_empty());
    }

    #[test]
    fn event_overflow_discards_queued_acceptance_authority() {
        let (client, backend) = AuthClient::test_bridge(1);
        backend
            .event(AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Accepted,
            })
            .unwrap();
        assert!(
            backend
                .event(AuthEvent {
                    request_id: 1,
                    outcome: AuthOutcome::Revoked
                })
                .is_err()
        );
        assert!(client.drain_events().is_err());
        assert!(client.end(1).is_err());
    }

    #[test]
    fn dead_backend_revokes_queued_success_and_cannot_allocate() {
        let (client, backend) = AuthClient::test_bridge(2);
        backend
            .event(AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Accepted,
            })
            .unwrap();
        drop(backend);
        assert!(client.drain_events().is_err());
        assert!(client.allocate_request_id().is_err());
    }

    #[test]
    fn explicit_failure_and_id_exhaustion_fail_closed() {
        let (client, backend) = AuthClient::test_bridge(2);
        assert_eq!(client.allocate_request_id().unwrap(), 1);
        assert_eq!(client.clone().allocate_request_id().unwrap(), 2);
        client
            .shared
            .next_request
            .store(u64::MAX, Ordering::Release);
        assert!(client.allocate_request_id().is_err());
        assert!(client.drain_events().is_err());
        let (client, backend2) = AuthClient::test_bridge(2);
        backend2.fail();
        assert!(client.drain_events().is_err());
        drop(backend);
    }

    #[test]
    fn events_are_drained_once_in_order() {
        let (client, backend) = AuthClient::test_bridge(2);
        let events = [
            AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Accepted,
            },
            AuthEvent {
                request_id: 1,
                outcome: AuthOutcome::Revoked,
            },
        ];
        for event in events {
            backend.event(event).unwrap();
        }
        assert_eq!(client.drain_events().unwrap(), events);
        assert!(client.drain_events().unwrap().is_empty());
    }
}
