//! TLS setup, HTTP agent, CM directory lookup and CDN downloads.
//!
//! TLS uses rustls with the ring provider and the host's root store. Every
//! HTTP call has connect, per-read and overall timeouts and a body size
//! bound; CDN requests rotate through hosts a bounded number of times.

use std::{sync::Arc, thread, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use rustls::{ClientConfig, RootCertStore, crypto::CryptoProvider, pki_types::CertificateDer};
use serde::Deserialize;

use super::{chunk::ChunkSpec, proto::ContentServerInfo};

/// Steam Web API directory of CM servers.
pub const CM_DIRECTORY_URL: &str =
    "https://api.steampowered.com/ISteamDirectory/GetCMListForConnect/v1/?cellid=0";
const USER_AGENT: &str = concat!("icarus-fetch/", env!("CARGO_PKG_VERSION"));
const MAX_DIRECTORY_BYTES: u64 = 1024 * 1024;

/// Shared TLS material for the CM WebSocket and the HTTP agent.
#[derive(Clone)]
pub struct Tls {
    pub provider: Arc<CryptoProvider>,
    pub roots: Vec<CertificateDer<'static>>,
    pub client: Arc<ClientConfig>,
}

impl Tls {
    /// Load the host's root certificates and build a ring-backed config.
    pub fn from_native_roots() -> Result<Self> {
        let loaded = rustls_native_certs::load_native_certs();
        let tls = Self::from_roots(loaded.certs)?;
        ensure!(
            !tls.roots.is_empty(),
            "no usable system root certificates ({} load errors)",
            loaded.errors.len()
        );
        Ok(tls)
    }

    /// Build a ring-backed config trusting `roots`. Unparsable certificates
    /// are dropped; an empty store is allowed (no TLS peer then verifies).
    pub fn from_roots(roots: Vec<CertificateDer<'static>>) -> Result<Self> {
        let mut store = RootCertStore::empty();
        let mut usable = Vec::with_capacity(roots.len());
        for root in roots {
            if store.add(root.clone()).is_ok() {
                usable.push(root);
            }
        }
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let client = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .context("TLS protocol versions")?
            .with_root_certificates(store)
            .with_no_client_auth();
        Ok(Self {
            provider,
            roots: usable,
            client: Arc::new(client),
        })
    }
}

/// Timeouts for HTTP calls.
#[derive(Debug, Clone, Copy)]
pub struct HttpTimeouts {
    pub connect: Duration,
    pub per_read: Duration,
    pub overall: Duration,
}

impl Default for HttpTimeouts {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(15),
            per_read: Duration::from_secs(30),
            overall: Duration::from_secs(300),
        }
    }
}

/// Build the blocking HTTP agent.
pub fn http_agent(tls: &Tls, timeouts: &HttpTimeouts) -> ureq::Agent {
    let roots: Vec<ureq::tls::Certificate<'static>> = tls
        .roots
        .iter()
        .map(|der| ureq::tls::Certificate::from_der(der.as_ref()).to_owned())
        .collect();
    let tls_config = ureq::tls::TlsConfig::builder()
        .provider(ureq::tls::TlsProvider::Rustls)
        .root_certs(ureq::tls::RootCerts::new_with_certs(&roots))
        .unversioned_rustls_crypto_provider(tls.provider.clone())
        .build();
    ureq::Agent::config_builder()
        .tls_config(tls_config)
        .timeout_connect(Some(timeouts.connect))
        .timeout_per_read(Some(timeouts.per_read))
        .timeout_global(Some(timeouts.overall))
        .max_redirects(3)
        .user_agent(USER_AGENT)
        .build()
        .new_agent()
}

/// GET `url` and return at most `max` body bytes.
pub fn get_bounded(agent: &ureq::Agent, url: &str, max: u64) -> Result<Vec<u8>> {
    let mut response = agent
        .get(url)
        .call()
        .with_context(|| format!("GET {url}"))?;
    // ureq rejects a body whose length reaches its limit, so allow one byte
    // more and bound the result ourselves.
    let body = response
        .body_mut()
        .with_config()
        .limit(max.saturating_add(1))
        .read_to_vec()
        .with_context(|| format!("read body of {url}"))?;
    anyhow::ensure!(
        body.len() as u64 <= max,
        "body of {url} exceeds {max} bytes"
    );
    Ok(body)
}

#[derive(Deserialize)]
struct DirectoryEnvelope {
    response: DirectoryResponse,
}

#[derive(Deserialize)]
struct DirectoryResponse {
    #[serde(default)]
    serverlist: Vec<DirectoryServer>,
    #[serde(default)]
    success: bool,
}

#[derive(Deserialize)]
struct DirectoryServer {
    endpoint: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    realm: Option<String>,
}

/// Parse a `GetCMListForConnect` response into WebSocket endpoints
/// (`host:port`), keeping directory order.
pub fn parse_cm_list(body: &[u8]) -> Result<Vec<String>> {
    let envelope: DirectoryEnvelope =
        serde_json::from_slice(body).map_err(|e| crate::json_parse_error("CM directory", e))?;
    ensure!(envelope.response.success, "CM directory reported failure");
    let endpoints: Vec<String> = envelope
        .response
        .serverlist
        .into_iter()
        .filter(|server| server.kind == "websockets")
        .filter(|server| {
            server
                .realm
                .as_deref()
                .is_none_or(|realm| realm == "steamglobal")
        })
        .map(|server| server.endpoint)
        .collect();
    ensure!(
        !endpoints.is_empty(),
        "CM directory lists no WebSocket servers"
    );
    Ok(endpoints)
}

/// Fetch WebSocket CM endpoints from the Steam directory.
pub fn cm_endpoints(agent: &ureq::Agent) -> Result<Vec<String>> {
    parse_cm_list(&get_bounded(agent, CM_DIRECTORY_URL, MAX_DIRECTORY_BYTES)?)
}

/// A CDN host usable for depot downloads; always reached over HTTPS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdnServer {
    pub host: String,
}

impl CdnServer {
    pub fn url(&self, path: &str) -> String {
        format!("https://{}/{path}", self.host)
    }
}

/// Keep direct CDN and cache servers that offer HTTPS and serve `app_id`,
/// lightest first. Hosts without HTTPS are never used.
pub fn select_cdn_servers(servers: &[ContentServerInfo], app_id: u32) -> Vec<CdnServer> {
    let mut usable: Vec<&ContentServerInfo> = servers
        .iter()
        .filter(|s| matches!(s.r#type.as_deref(), Some("CDN" | "SteamCache")))
        .filter(|s| matches!(s.https_support.as_deref(), Some("mandatory" | "optional")))
        .filter(|s| !s.use_as_proxy.unwrap_or(false) && !s.steam_china_only.unwrap_or(false))
        .filter(|s| s.allowed_app_ids.is_empty() || s.allowed_app_ids.contains(&app_id))
        .filter(|s| {
            s.vhost
                .as_deref()
                .or(s.host.as_deref())
                .is_some_and(valid_host)
        })
        .collect();
    usable.sort_by(|a, b| {
        a.priority_class
            .unwrap_or(0)
            .cmp(&b.priority_class.unwrap_or(0))
            .then(
                a.weighted_load
                    .unwrap_or(0.0)
                    .total_cmp(&b.weighted_load.unwrap_or(0.0)),
            )
    });
    let mut out: Vec<CdnServer> = Vec::new();
    for server in usable {
        let host = server
            .vhost
            .as_deref()
            .or(server.host.as_deref())
            .unwrap_or_default()
            .to_owned();
        if !out.iter().any(|known| known.host == host) {
            out.push(CdnServer { host });
        }
    }
    out
}

/// Accept only plain host names (optionally with a port).
fn valid_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 253
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-' | b':'))
}

/// Size bounds for CDN bodies.
#[derive(Debug, Clone, Copy)]
pub struct CdnLimits {
    pub max_manifest_download: u64,
    pub max_chunk_download: u64,
    /// Total attempts per object across hosts, download and validation
    /// together.
    pub max_attempts: usize,
    /// Pause before retry `n` is `n` times this.
    pub retry_backoff: Duration,
}

impl Default for CdnLimits {
    fn default() -> Self {
        Self {
            max_manifest_download: 128 * 1024 * 1024,
            max_chunk_download: 64 * 1024 * 1024,
            max_attempts: 6,
            retry_backoff: Duration::from_millis(250),
        }
    }
}

/// Run `attempt` against host indices `start`, `start + 1`, ... (modulo
/// `hosts`) until it succeeds, at most `max_attempts` times. Each attempt is
/// a whole fetch-and-validate step, so a corrupt body from one host is
/// retried on the next.
pub fn retry_across_hosts<T>(
    hosts: usize,
    max_attempts: usize,
    start: usize,
    backoff: Duration,
    mut attempt: impl FnMut(usize) -> Result<T>,
) -> Result<T> {
    ensure!(hosts > 0, "no usable CDN servers");
    let attempts = max_attempts.max(1);
    let mut errors = Vec::new();
    for round in 0..attempts {
        if round > 0 && !backoff.is_zero() {
            thread::sleep(backoff.saturating_mul(round as u32));
        }
        let host = (start.wrapping_add(round)) % hosts;
        match attempt(host) {
            Ok(value) => return Ok(value),
            Err(error) => errors.push(format!("host {host}: {error:#}")),
        }
    }
    bail!("failed after {attempts} attempt(s): {}", errors.join("; "))
}

/// Download depot objects with bounded host rotation.
pub struct Cdn {
    pub agent: ureq::Agent,
    pub servers: Vec<CdnServer>,
    pub limits: CdnLimits,
}

impl Cdn {
    /// GET `path` and pass the body to `validate`; a transport or validation
    /// failure moves on to the next host.
    pub fn get_validated<T>(
        &self,
        path: &str,
        max: u64,
        start: usize,
        validate: impl Fn(Vec<u8>) -> Result<T>,
    ) -> Result<T> {
        retry_across_hosts(
            self.servers.len(),
            self.limits.max_attempts,
            start,
            self.limits.retry_backoff,
            |host| {
                let server = &self.servers[host];
                let body = get_bounded(&self.agent, &server.url(path), max)?;
                validate(body).with_context(|| format!("validate {path} from {}", server.host))
            },
        )
        .with_context(|| format!("CDN {path}"))
    }

    /// Download a zipped manifest and decode it with `decode`; `request_code`
    /// 0 omits the code segment.
    pub fn manifest<T>(
        &self,
        depot_id: u32,
        gid: u64,
        request_code: u64,
        decode: impl Fn(Vec<u8>) -> Result<T>,
    ) -> Result<T> {
        let path = if request_code == 0 {
            format!("depot/{depot_id}/manifest/{gid}/5")
        } else {
            format!("depot/{depot_id}/manifest/{gid}/5/{request_code}")
        };
        self.get_validated(&path, self.limits.max_manifest_download, 0, decode)
    }

    /// Download one encrypted chunk and decode it with `decode`; `start`
    /// spreads workers across hosts.
    pub fn chunk<T>(
        &self,
        depot_id: u32,
        chunk: &ChunkSpec,
        start: usize,
        decode: impl Fn(Vec<u8>) -> Result<T>,
    ) -> Result<T> {
        let max = if chunk.compressed_len > 0 {
            u64::from(chunk.compressed_len)
        } else {
            self.limits.max_chunk_download
        };
        ensure!(
            max <= self.limits.max_chunk_download,
            "chunk {} compressed size {max} exceeds the limit",
            chunk.id_hex()
        );
        self.get_validated(
            &format!("depot/{depot_id}/chunk/{}", chunk.id_hex()),
            max,
            start,
            decode,
        )
    }
}
