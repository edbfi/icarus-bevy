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
        let mut store = RootCertStore::empty();
        let (added, _ignored) = store.add_parsable_certificates(loaded.certs.iter().cloned());
        ensure!(
            added > 0,
            "no usable system root certificates ({} load errors)",
            loaded.errors.len()
        );
        let provider = Arc::new(rustls::crypto::ring::default_provider());
        let client = ClientConfig::builder_with_provider(provider.clone())
            .with_safe_default_protocol_versions()
            .context("TLS protocol versions")?
            .with_root_certificates(store)
            .with_no_client_auth();
        Ok(Self {
            provider,
            roots: loaded.certs,
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

/// A CDN host usable for depot downloads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdnServer {
    pub host: String,
    pub https: bool,
}

impl CdnServer {
    pub fn url(&self, path: &str) -> String {
        let scheme = if self.https { "https" } else { "http" };
        format!("{scheme}://{}/{path}", self.host)
    }
}

/// Keep direct CDN and cache servers usable for `app_id`, lightest first.
pub fn select_cdn_servers(servers: &[ContentServerInfo], app_id: u32) -> Vec<CdnServer> {
    let mut usable: Vec<&ContentServerInfo> = servers
        .iter()
        .filter(|s| matches!(s.r#type.as_deref(), Some("CDN" | "SteamCache")))
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
        let https = matches!(
            server.https_support.as_deref(),
            Some("mandatory" | "optional")
        );
        if !out.iter().any(|known| known.host == host) {
            out.push(CdnServer { host, https });
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
    /// Total attempts per object across hosts.
    pub max_attempts: usize,
}

impl Default for CdnLimits {
    fn default() -> Self {
        Self {
            max_manifest_download: 128 * 1024 * 1024,
            max_chunk_download: 64 * 1024 * 1024,
            max_attempts: 6,
        }
    }
}

/// Download depot objects with bounded host rotation.
pub struct Cdn {
    pub agent: ureq::Agent,
    pub servers: Vec<CdnServer>,
    pub limits: CdnLimits,
}

impl Cdn {
    fn get(&self, path: &str, max: u64, start: usize) -> Result<Vec<u8>> {
        ensure!(!self.servers.is_empty(), "no usable CDN servers");
        let mut last_error = None;
        for attempt in 0..self.limits.max_attempts.max(1) {
            if attempt > 0 {
                thread::sleep(Duration::from_millis(250 * attempt as u64));
            }
            let server = &self.servers[(start + attempt) % self.servers.len()];
            match get_bounded(&self.agent, &server.url(path), max) {
                Ok(body) => return Ok(body),
                Err(error) => last_error = Some(error),
            }
        }
        match last_error {
            Some(error) => Err(error.context(format!(
                "{path} failed after {} attempts",
                self.limits.max_attempts.max(1)
            ))),
            None => bail!("{path}: no attempt made"),
        }
    }

    /// Download a zipped manifest; `request_code` 0 omits the code segment.
    pub fn manifest(&self, depot_id: u32, gid: u64, request_code: u64) -> Result<Vec<u8>> {
        let path = if request_code == 0 {
            format!("depot/{depot_id}/manifest/{gid}/5")
        } else {
            format!("depot/{depot_id}/manifest/{gid}/5/{request_code}")
        };
        self.get(&path, self.limits.max_manifest_download, 0)
    }

    /// Download one encrypted chunk; `start` spreads workers across hosts.
    pub fn chunk(&self, depot_id: u32, chunk: &ChunkSpec, start: usize) -> Result<Vec<u8>> {
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
        self.get(
            &format!("depot/{depot_id}/chunk/{}", chunk.id_hex()),
            max,
            start,
        )
    }
}
