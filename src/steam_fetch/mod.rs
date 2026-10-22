//! Anonymous SteamPipe content fetcher.
//!
//! Fetches one file from a public Steam app's depot without steamcmd or
//! credentials: CM directory, TLS WebSocket CM session with anonymous logon,
//! PICS app info, depot key, CDN host list and manifest request code, then
//! the zipped manifest and the chunks of the requested file from the CDN.
//! Nothing secret is persisted. Steam's documented/observed protocol
//! (SteamKit2 as reference, independently implemented).

pub mod assemble;
pub mod chunk;
pub mod cm;
pub mod frame;
pub mod manifest;
pub mod net;
pub mod proto;
pub mod vdf;

use std::path::Path;

use anyhow::{Context, Result, bail, ensure};
use prost::Message as _;

use self::{
    assemble::{AssembledFile, AssemblyLimits},
    cm::{CmLimits, CmSession, CmTimeouts, ERESULT_OK, eresult_name},
    frame::emsg,
    manifest::{Manifest, ManifestLimits},
    net::{Cdn, CdnLimits, HttpTimeouts, Tls},
    proto::{
        GetDepotDecryptionKey, GetDepotDecryptionKeyResponse, GetManifestRequestCodeRequest,
        GetManifestRequestCodeResponse, GetServersForSteamPipeRequest,
        GetServersForSteamPipeResponse, PicsAccessTokenRequest, PicsAccessTokenResponse,
        PicsAppRequest, PicsProductInfoRequest, PicsProductInfoResponse,
    },
};

/// CM endpoints tried before giving up.
const MAX_CM_ATTEMPTS: usize = 4;
/// Depots whose manifests are inspected while looking for the file.
const MAX_DEPOT_CANDIDATES: usize = 8;
/// PICS product-info responses accepted for one request.
const MAX_PICS_RESPONSES: usize = 16;
/// Bound on the app-info text buffer.
const MAX_APP_INFO_BYTES: usize = 16 * 1024 * 1024;

/// Lowercase hex of a digest.
pub fn hex(bytes: &[u8]) -> String {
    crate::hex_lower(bytes)
}

/// What to fetch and the bounds to apply.
#[derive(Debug, Clone)]
pub struct FetchRequest {
    pub app_id: u32,
    pub branch: String,
    pub file: String,
    /// Restrict the search to one depot.
    pub depot: Option<u32>,
    pub cm: CmTimeouts,
    pub cm_limits: CmLimits,
    pub http: HttpTimeouts,
    pub cdn: CdnLimits,
    pub manifest: ManifestLimits,
    pub assembly: AssemblyLimits,
    pub workers: usize,
}

impl FetchRequest {
    pub fn new(app_id: u32, branch: &str, file: &str) -> Self {
        Self {
            app_id,
            branch: branch.to_owned(),
            file: file.to_owned(),
            depot: None,
            cm: CmTimeouts::default(),
            cm_limits: CmLimits::default(),
            http: HttpTimeouts::default(),
            cdn: CdnLimits::default(),
            manifest: ManifestLimits::default(),
            assembly: AssemblyLimits::default(),
            workers: 4,
        }
    }
}

/// The depot and manifest holding the requested file, with what is needed
/// to download it. The depot key lives only in memory.
pub struct Resolved {
    pub build_id: Option<u64>,
    pub depot_id: u32,
    pub manifest_gid: u64,
    pub manifest: Manifest,
    depot_key: Vec<u8>,
    cdn: Cdn,
}

impl Resolved {
    /// The requested file's manifest entry.
    pub fn file<'a>(&'a self, request: &FetchRequest) -> Result<&'a manifest::ManifestFile> {
        self.manifest
            .find(&request.file)
            .with_context(|| format!("{} is not in depot {}", request.file, self.depot_id))
    }

    /// Download, verify and atomically write the requested file.
    pub fn download(&self, request: &FetchRequest, out_dir: &Path) -> Result<AssembledFile> {
        let file = self.file(request)?;
        assemble::assemble(out_dir, file, &request.assembly, request.workers, |chunk| {
            let start = usize::from(chunk.id[0]);
            let raw = self.cdn.chunk(self.depot_id, chunk, start)?;
            chunk::decode_chunk(&raw, &self.depot_key, chunk)
        })
    }
}

fn connect_and_logon(
    tls: &Tls,
    agent: &ureq::Agent,
    request: &FetchRequest,
    progress: &dyn Fn(&str),
) -> Result<(CmSession, cm::Logon)> {
    let endpoints = net::cm_endpoints(agent).context("CM directory")?;
    let mut failures = Vec::new();
    for endpoint in endpoints.iter().take(MAX_CM_ATTEMPTS) {
        progress(&format!("connecting to CM {endpoint}"));
        let attempt =
            CmSession::connect(endpoint, tls.client.clone(), request.cm, request.cm_limits)
                .and_then(|mut session| {
                    let logon = session.logon_anonymous()?;
                    Ok((session, logon))
                });
        match attempt {
            Ok(connected) => return Ok(connected),
            Err(error) => failures.push(format!("{endpoint}: {error:#}")),
        }
    }
    bail!("no CM accepted an anonymous logon: {}", failures.join("; "))
}

/// Fetch and parse the app's PICS app info.
fn app_info(session: &mut CmSession, app_id: u32) -> Result<vdf::Value> {
    let token_request = PicsAccessTokenRequest {
        packageids: Vec::new(),
        appids: vec![app_id],
    };
    let packet = session.request(
        emsg::CLIENT_PICS_ACCESS_TOKEN_REQUEST,
        &token_request.encode_to_vec(),
        emsg::CLIENT_PICS_ACCESS_TOKEN_RESPONSE,
    )?;
    let tokens = PicsAccessTokenResponse::decode(packet.body.as_slice())
        .context("PICS access token response")?;
    let token = tokens
        .app_access_tokens
        .iter()
        .find(|token| token.appid == Some(app_id))
        .and_then(|token| token.access_token)
        .unwrap_or(0);

    let info_request = PicsProductInfoRequest {
        apps: vec![PicsAppRequest {
            appid: Some(app_id),
            access_token: Some(token),
        }],
        meta_data_only: Some(false),
    };
    let job = session.send_job(
        emsg::CLIENT_PICS_PRODUCT_INFO_REQUEST,
        &info_request.encode_to_vec(),
        None,
    )?;
    let mut buffer = None;
    for _ in 0..MAX_PICS_RESPONSES {
        let packet = session.recv_job(job, emsg::CLIENT_PICS_PRODUCT_INFO_RESPONSE)?;
        let response = PicsProductInfoResponse::decode(packet.body.as_slice())
            .context("PICS product info response")?;
        ensure!(
            !response.unknown_appids.contains(&app_id),
            "PICS does not know app {app_id}"
        );
        for app in response.apps {
            if app.appid == Some(app_id) {
                ensure!(
                    !app.missing_token.unwrap_or(false) || app.buffer.is_some(),
                    "PICS app info for {app_id} needs an access token anonymous users lack"
                );
                if let Some(bytes) = app.buffer {
                    buffer = Some(bytes);
                } else if let Some(host) = response.http_host.as_deref() {
                    bail!("PICS app info for {app_id} is only offered over HTTP from {host}");
                }
            }
        }
        if !response.response_pending.unwrap_or(false) {
            break;
        }
    }
    let buffer = buffer.with_context(|| format!("PICS returned no app info for {app_id}"))?;
    ensure!(
        buffer.len() <= MAX_APP_INFO_BYTES,
        "app info exceeds {MAX_APP_INFO_BYTES} bytes"
    );
    vdf::parse(&buffer).context("parse app info KeyValues")
}

fn depot_key(session: &mut CmSession, app_id: u32, depot_id: u32) -> Result<Vec<u8>> {
    let body = GetDepotDecryptionKey {
        depot_id: Some(depot_id),
        app_id: Some(app_id),
    };
    let packet = session.request(
        emsg::CLIENT_GET_DEPOT_DECRYPTION_KEY,
        &body.encode_to_vec(),
        emsg::CLIENT_GET_DEPOT_DECRYPTION_KEY_RESPONSE,
    )?;
    let response = GetDepotDecryptionKeyResponse::decode(packet.body.as_slice())
        .context("depot key response")?;
    let eresult = response.eresult.unwrap_or(2);
    ensure!(
        eresult == ERESULT_OK,
        "depot {depot_id} key denied: eresult {eresult} ({})",
        eresult_name(eresult)
    );
    let key = response.depot_encryption_key.unwrap_or_default();
    ensure!(
        key.len() == chunk::KEY_LEN,
        "depot {depot_id} key is {} bytes",
        key.len()
    );
    Ok(key)
}

/// Log on anonymously and find the depot whose current manifest holds the
/// requested file. Downloads only metadata (app info and manifests).
pub fn resolve(request: &FetchRequest, progress: &dyn Fn(&str)) -> Result<Resolved> {
    let tls = Tls::from_native_roots()?;
    let agent = net::http_agent(&tls, &request.http);
    let (mut session, logon) = connect_and_logon(&tls, &agent, request, progress)?;
    progress(&format!(
        "anonymous logon OK (cell {}, heartbeat {}s)",
        logon.cell_id,
        logon.heartbeat.as_secs()
    ));
    let result = resolve_in_session(&mut session, logon, agent, request, progress);
    session.close();
    result
}

fn resolve_in_session(
    session: &mut CmSession,
    logon: cm::Logon,
    agent: ureq::Agent,
    request: &FetchRequest,
    progress: &dyn Fn(&str),
) -> Result<Resolved> {
    let info = app_info(session, request.app_id)?;
    let depots = vdf::app_depots(&info, &request.branch)?;
    progress(&format!(
        "app {} branch {} build {:?}: {} depot(s) with manifests",
        request.app_id,
        request.branch,
        depots.build_id,
        depots.depots.len()
    ));
    let mut candidates = vdf::windows_first(&depots.depots);
    if let Some(depot) = request.depot {
        candidates.retain(|entry| entry.depot_id == depot);
        ensure!(
            !candidates.is_empty(),
            "depot {depot} has no {} manifest",
            request.branch
        );
    }
    ensure!(!candidates.is_empty(), "app lists no depot manifests");

    let servers: GetServersForSteamPipeResponse = session.service_call(
        "ContentServerDirectory.GetServersForSteamPipe#1",
        &GetServersForSteamPipeRequest {
            cell_id: Some(logon.cell_id),
            max_servers: Some(20),
        },
    )?;
    let cdn_servers = net::select_cdn_servers(&servers.servers, request.app_id);
    ensure!(
        !cdn_servers.is_empty(),
        "no usable CDN servers among {}",
        servers.servers.len()
    );
    progress(&format!("{} CDN server(s)", cdn_servers.len()));
    let cdn = Cdn {
        agent,
        servers: cdn_servers,
        limits: request.cdn,
    };

    let mut notes = Vec::new();
    for entry in candidates.iter().take(MAX_DEPOT_CANDIDATES) {
        let key = match depot_key(session, request.app_id, entry.depot_id) {
            Ok(key) => key,
            Err(error) => {
                notes.push(format!("{}: {error:#}", entry.depot_id));
                continue;
            }
        };
        let code: GetManifestRequestCodeResponse = session.service_call(
            "ContentServerDirectory.GetManifestRequestCode#1",
            &GetManifestRequestCodeRequest {
                app_id: Some(request.app_id),
                depot_id: Some(entry.depot_id),
                manifest_id: Some(entry.manifest_gid),
                app_branch: (!request.branch.eq_ignore_ascii_case("public"))
                    .then(|| request.branch.clone()),
            },
        )?;
        let code = code.manifest_request_code.unwrap_or(0);
        progress(&format!(
            "depot {} manifest {}: request code {}",
            entry.depot_id,
            entry.manifest_gid,
            if code == 0 { "absent" } else { "received" }
        ));
        let zipped = cdn.manifest(entry.depot_id, entry.manifest_gid, code)?;
        let manifest = manifest::decode_zipped(&zipped, Some(&key), &request.manifest)
            .with_context(|| format!("depot {} manifest", entry.depot_id))?;
        ensure!(
            manifest.depot_id == entry.depot_id && manifest.gid == entry.manifest_gid,
            "manifest metadata names depot {} gid {}, expected {} {}",
            manifest.depot_id,
            manifest.gid,
            entry.depot_id,
            entry.manifest_gid
        );
        if manifest.find(&request.file).is_some() {
            return Ok(Resolved {
                build_id: depots.build_id,
                depot_id: entry.depot_id,
                manifest_gid: entry.manifest_gid,
                manifest,
                depot_key: key,
                cdn,
            });
        }
        notes.push(format!("{}: file not in manifest", entry.depot_id));
    }
    bail!("no depot holds {}: {}", request.file, notes.join("; "))
}
