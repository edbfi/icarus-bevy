//! Minimal hand-written protobuf messages for anonymous SteamPipe access.
//!
//! Field numbers follow Steam's documented/observed protocol (SteamKit2 and
//! SteamDatabase/Protobufs as reference, independently implemented). Only the
//! fields this fetcher reads or writes are declared; prost skips the rest.

/// `CMsgProtoBufHeader`, the header of every protobuf-flagged CM message.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ProtoHeader {
    #[prost(fixed64, optional, tag = "1")]
    pub steamid: Option<u64>,
    #[prost(int32, optional, tag = "2")]
    pub client_sessionid: Option<i32>,
    #[prost(uint32, optional, tag = "3")]
    pub routing_appid: Option<u32>,
    /// Absent means the protocol default `u64::MAX` (no job).
    #[prost(fixed64, optional, tag = "10")]
    pub jobid_source: Option<u64>,
    #[prost(fixed64, optional, tag = "11")]
    pub jobid_target: Option<u64>,
    #[prost(string, optional, tag = "12")]
    pub target_job_name: Option<String>,
    /// Absent means the protocol default 2 (`Fail`).
    #[prost(int32, optional, tag = "13")]
    pub eresult: Option<i32>,
    #[prost(string, optional, tag = "14")]
    pub error_message: Option<String>,
}

/// `CMsgMulti`: several length-prefixed messages, gzip-compressed when
/// `size_unzipped` is non-zero.
#[derive(Clone, PartialEq, prost::Message)]
pub struct Multi {
    #[prost(uint32, optional, tag = "1")]
    pub size_unzipped: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "2")]
    pub message_body: Option<Vec<u8>>,
}

/// `CMsgClientHello`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ClientHello {
    #[prost(uint32, optional, tag = "1")]
    pub protocol_version: Option<u32>,
}

/// `CMsgClientHeartBeat`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ClientHeartBeat {
    #[prost(bool, optional, tag = "1")]
    pub send_reply: Option<bool>,
}

/// `CMsgClientLogon`, reduced to the fields an anonymous logon sets.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ClientLogon {
    #[prost(uint32, optional, tag = "1")]
    pub protocol_version: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub cell_id: Option<u32>,
    #[prost(string, optional, tag = "6")]
    pub client_language: Option<String>,
    #[prost(uint32, optional, tag = "7")]
    pub client_os_type: Option<u32>,
    /// Game servers send their private address XOR [`PRIVATE_IP_MASK`].
    #[prost(message, optional, tag = "11")]
    pub obfuscated_private_ip: Option<IpAddress>,
    /// The application a game server serves; absent for user logons.
    #[prost(int32, optional, tag = "94")]
    pub game_server_app_id: Option<i32>,
}

/// The XOR mask Steam clients apply to a reported private IPv4 address.
pub const PRIVATE_IP_MASK: u32 = 0xBAAD_F00D;

/// `CMsgIPAddress`, IPv4 form only.
#[derive(Clone, PartialEq, prost::Message)]
pub struct IpAddress {
    #[prost(fixed32, optional, tag = "1")]
    pub v4: Option<u32>,
}

/// `CMsgGSServerType`: what a logged-on game server serves and where.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GsServerType {
    #[prost(uint32, optional, tag = "1")]
    pub app_id_served: Option<u32>,
    /// `EServerFlags`: active 1, secure 2, dedicated 4, Linux 8, passworded 16.
    #[prost(uint32, optional, tag = "2")]
    pub flags: Option<u32>,
    #[prost(uint32, optional, tag = "4")]
    pub game_port: Option<u32>,
    #[prost(string, optional, tag = "5")]
    pub game_dir: Option<String>,
    #[prost(string, optional, tag = "6")]
    pub game_version: Option<String>,
    #[prost(uint32, optional, tag = "7")]
    pub game_query_port: Option<u32>,
}

/// `CMsgGSStatusReply`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GsStatusReply {
    #[prost(bool, optional, tag = "1")]
    pub is_secure: Option<bool>,
}

/// Auth protocol field numbers follow SteamDatabase/Protobufs, revision
/// `1ad9f323b1ace1b76e5052fef141edccd11acb95`, steam/steammessages_base.proto
/// and steam/steammessages_clientserver.proto.
/// Ticket material is deliberately omitted by Debug.
#[derive(Clone, PartialEq, prost::Message)]
#[prost(skip_debug)]
pub struct AuthTicket {
    #[prost(uint32, optional, tag = "1")]
    pub estate: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub eresult: Option<u32>,
    #[prost(fixed64, optional, tag = "3")]
    pub steamid: Option<u64>,
    #[prost(fixed64, optional, tag = "4")]
    pub gameid: Option<u64>,
    #[prost(uint32, optional, tag = "5")]
    pub h_steam_pipe: Option<u32>,
    #[prost(uint32, optional, tag = "6")]
    pub ticket_crc: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "7")]
    pub ticket: Option<Vec<u8>>,
}

impl std::fmt::Debug for AuthTicket {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("AuthTicket([REDACTED])")
    }
}

/// Complete active authentication list; an ACK does not verify a peer.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ClientAuthList {
    #[prost(uint32, optional, tag = "1")]
    pub tokens_left: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub last_request_seq: Option<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub last_request_seq_from_server: Option<u32>,
    #[prost(message, repeated, tag = "4")]
    pub tickets: Vec<AuthTicket>,
    #[prost(uint32, repeated, packed = "false", tag = "5")]
    pub app_ids: Vec<u32>,
    #[prost(uint32, optional, tag = "6")]
    pub message_sequence: Option<u32>,
}

/// List receipt only. Sequence/job echoing is not a reliable validation signal.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ClientAuthListAck {
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub ticket_crc: Vec<u32>,
    #[prost(uint32, repeated, packed = "false", tag = "2")]
    pub app_ids: Vec<u32>,
    #[prost(uint32, optional, tag = "3")]
    pub message_sequence: Option<u32>,
}

/// Asynchronous validation/revocation; absent response must not default to OK.
#[derive(Clone, PartialEq, prost::Message)]
#[prost(skip_debug)]
pub struct ClientTicketAuthComplete {
    #[prost(fixed64, optional, tag = "1")]
    pub steam_id: Option<u64>,
    #[prost(fixed64, optional, tag = "2")]
    pub game_id: Option<u64>,
    #[prost(uint32, optional, tag = "3")]
    pub estate: Option<u32>,
    #[prost(uint32, optional, tag = "4")]
    pub eauth_session_response: Option<u32>,
    #[prost(uint32, optional, tag = "6")]
    pub ticket_crc: Option<u32>,
    #[prost(uint32, optional, tag = "7")]
    pub ticket_sequence: Option<u32>,
    #[prost(fixed64, optional, tag = "8")]
    pub owner_steam_id: Option<u64>,
}

impl std::fmt::Debug for ClientTicketAuthComplete {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("ClientTicketAuthComplete([REDACTED])")
    }
}

/// `CMsgGameServerData.Player`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GameServerPlayer {
    #[prost(fixed64, optional, tag = "1")]
    pub steam_id: Option<u64>,
}

/// `CMsgGameServerData`: the details a server browser lists.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GameServerData {
    #[prost(uint32, optional, tag = "3")]
    pub query_port: Option<u32>,
    #[prost(uint32, optional, tag = "4")]
    pub game_port: Option<u32>,
    #[prost(uint32, optional, tag = "5")]
    pub spectator_port: Option<u32>,
    #[prost(uint32, optional, tag = "6")]
    pub app_id: Option<u32>,
    #[prost(string, optional, tag = "7")]
    pub gamedir: Option<String>,
    #[prost(string, optional, tag = "8")]
    pub version: Option<String>,
    #[prost(string, optional, tag = "9")]
    pub product: Option<String>,
    #[prost(string, optional, tag = "10")]
    pub region: Option<String>,
    #[prost(message, repeated, tag = "11")]
    pub players: Vec<GameServerPlayer>,
    #[prost(uint32, optional, tag = "12")]
    pub max_players: Option<u32>,
    #[prost(uint32, optional, tag = "13")]
    pub bot_count: Option<u32>,
    #[prost(bool, optional, tag = "14")]
    pub password: Option<bool>,
    #[prost(bool, optional, tag = "15")]
    pub secure: Option<bool>,
    #[prost(bool, optional, tag = "16")]
    pub dedicated: Option<bool>,
    #[prost(string, optional, tag = "17")]
    pub os: Option<String>,
    #[prost(string, optional, tag = "18")]
    pub game_data: Option<String>,
    #[prost(string, optional, tag = "20")]
    pub game_type: Option<String>,
    #[prost(string, optional, tag = "21")]
    pub map: Option<String>,
    #[prost(string, optional, tag = "22")]
    pub server_name: Option<String>,
    #[prost(uint32, optional, tag = "24")]
    pub revision: Option<u32>,
    #[prost(string, optional, tag = "29")]
    pub game_description: Option<String>,
}

/// `CMsgClientLogonResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ClientLogonResponse {
    #[prost(int32, optional, tag = "1")]
    pub eresult: Option<i32>,
    #[prost(int32, optional, tag = "2")]
    pub legacy_out_of_game_heartbeat_seconds: Option<i32>,
    #[prost(int32, optional, tag = "3")]
    pub heartbeat_seconds: Option<i32>,
    #[prost(uint32, optional, tag = "7")]
    pub cell_id: Option<u32>,
    #[prost(int32, optional, tag = "10")]
    pub eresult_extended: Option<i32>,
}

/// `CMsgClientLoggedOff`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ClientLoggedOff {
    #[prost(int32, optional, tag = "1")]
    pub eresult: Option<i32>,
}

/// `CMsgClientPICSAccessTokenRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PicsAccessTokenRequest {
    #[prost(uint32, repeated, packed = "false", tag = "1")]
    pub packageids: Vec<u32>,
    #[prost(uint32, repeated, packed = "false", tag = "2")]
    pub appids: Vec<u32>,
}

/// `CMsgClientPICSAccessTokenResponse.AppToken`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PicsAppToken {
    #[prost(uint32, optional, tag = "1")]
    pub appid: Option<u32>,
    #[prost(uint64, optional, tag = "2")]
    pub access_token: Option<u64>,
}

/// `CMsgClientPICSAccessTokenResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PicsAccessTokenResponse {
    #[prost(message, repeated, tag = "3")]
    pub app_access_tokens: Vec<PicsAppToken>,
    #[prost(uint32, repeated, tag = "4")]
    pub app_denied_tokens: Vec<u32>,
}

/// `CMsgClientPICSProductInfoRequest.AppInfo`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PicsAppRequest {
    #[prost(uint32, optional, tag = "1")]
    pub appid: Option<u32>,
    #[prost(uint64, optional, tag = "2")]
    pub access_token: Option<u64>,
}

/// `CMsgClientPICSProductInfoRequest`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PicsProductInfoRequest {
    #[prost(message, repeated, tag = "2")]
    pub apps: Vec<PicsAppRequest>,
    #[prost(bool, optional, tag = "3")]
    pub meta_data_only: Option<bool>,
}

/// `CMsgClientPICSProductInfoResponse.AppInfo`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PicsAppInfo {
    #[prost(uint32, optional, tag = "1")]
    pub appid: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub change_number: Option<u32>,
    #[prost(bool, optional, tag = "3")]
    pub missing_token: Option<bool>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub sha: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "5")]
    pub buffer: Option<Vec<u8>>,
    #[prost(uint32, optional, tag = "7")]
    pub size: Option<u32>,
}

/// `CMsgClientPICSProductInfoResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct PicsProductInfoResponse {
    #[prost(message, repeated, tag = "1")]
    pub apps: Vec<PicsAppInfo>,
    #[prost(uint32, repeated, tag = "2")]
    pub unknown_appids: Vec<u32>,
    #[prost(bool, optional, tag = "6")]
    pub response_pending: Option<bool>,
    #[prost(string, optional, tag = "8")]
    pub http_host: Option<String>,
}

/// `CMsgClientGetDepotDecryptionKey`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GetDepotDecryptionKey {
    #[prost(uint32, optional, tag = "1")]
    pub depot_id: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub app_id: Option<u32>,
}

/// `CMsgClientGetDepotDecryptionKeyResponse`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GetDepotDecryptionKeyResponse {
    #[prost(int32, optional, tag = "1")]
    pub eresult: Option<i32>,
    #[prost(uint32, optional, tag = "2")]
    pub depot_id: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "3")]
    pub depot_encryption_key: Option<Vec<u8>>,
}

/// `CContentServerDirectory_GetServersForSteamPipe_Request`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GetServersForSteamPipeRequest {
    #[prost(uint32, optional, tag = "1")]
    pub cell_id: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub max_servers: Option<u32>,
}

/// `CContentServerDirectory_ServerInfo`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ContentServerInfo {
    #[prost(string, optional, tag = "1")]
    pub r#type: Option<String>,
    #[prost(int32, optional, tag = "2")]
    pub source_id: Option<i32>,
    #[prost(int32, optional, tag = "3")]
    pub cell_id: Option<i32>,
    #[prost(int32, optional, tag = "4")]
    pub load: Option<i32>,
    #[prost(float, optional, tag = "5")]
    pub weighted_load: Option<f32>,
    #[prost(bool, optional, tag = "7")]
    pub steam_china_only: Option<bool>,
    #[prost(string, optional, tag = "8")]
    pub host: Option<String>,
    #[prost(string, optional, tag = "9")]
    pub vhost: Option<String>,
    #[prost(bool, optional, tag = "10")]
    pub use_as_proxy: Option<bool>,
    #[prost(string, optional, tag = "12")]
    pub https_support: Option<String>,
    #[prost(uint32, repeated, tag = "13")]
    pub allowed_app_ids: Vec<u32>,
    #[prost(uint32, optional, tag = "15")]
    pub priority_class: Option<u32>,
}

/// `CContentServerDirectory_GetServersForSteamPipe_Response`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GetServersForSteamPipeResponse {
    #[prost(message, repeated, tag = "1")]
    pub servers: Vec<ContentServerInfo>,
}

/// `CContentServerDirectory_GetManifestRequestCode_Request`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GetManifestRequestCodeRequest {
    #[prost(uint32, optional, tag = "1")]
    pub app_id: Option<u32>,
    #[prost(uint32, optional, tag = "2")]
    pub depot_id: Option<u32>,
    #[prost(uint64, optional, tag = "3")]
    pub manifest_id: Option<u64>,
    #[prost(string, optional, tag = "4")]
    pub app_branch: Option<String>,
}

/// `CContentServerDirectory_GetManifestRequestCode_Response`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct GetManifestRequestCodeResponse {
    #[prost(uint64, optional, tag = "1")]
    pub manifest_request_code: Option<u64>,
}

/// `ContentManifestPayload.FileMapping.ChunkData`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ManifestChunk {
    /// SHA-1 of the uncompressed chunk; also the chunk's CDN id.
    #[prost(bytes = "vec", optional, tag = "1")]
    pub sha: Option<Vec<u8>>,
    /// Adler-32 (seed 0) of the uncompressed chunk, despite the field name.
    #[prost(fixed32, optional, tag = "2")]
    pub crc: Option<u32>,
    #[prost(uint64, optional, tag = "3")]
    pub offset: Option<u64>,
    #[prost(uint32, optional, tag = "4")]
    pub cb_original: Option<u32>,
    #[prost(uint32, optional, tag = "5")]
    pub cb_compressed: Option<u32>,
}

/// `ContentManifestPayload.FileMapping`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ManifestFileMapping {
    #[prost(string, optional, tag = "1")]
    pub filename: Option<String>,
    #[prost(uint64, optional, tag = "2")]
    pub size: Option<u64>,
    #[prost(uint32, optional, tag = "3")]
    pub flags: Option<u32>,
    #[prost(bytes = "vec", optional, tag = "4")]
    pub sha_filename: Option<Vec<u8>>,
    #[prost(bytes = "vec", optional, tag = "5")]
    pub sha_content: Option<Vec<u8>>,
    #[prost(message, repeated, tag = "6")]
    pub chunks: Vec<ManifestChunk>,
    #[prost(string, optional, tag = "7")]
    pub linktarget: Option<String>,
}

/// `ContentManifestPayload`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ManifestPayload {
    #[prost(message, repeated, tag = "1")]
    pub mappings: Vec<ManifestFileMapping>,
}

/// `ContentManifestMetadata`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ManifestMetadata {
    #[prost(uint32, optional, tag = "1")]
    pub depot_id: Option<u32>,
    #[prost(uint64, optional, tag = "2")]
    pub gid_manifest: Option<u64>,
    #[prost(uint32, optional, tag = "3")]
    pub creation_time: Option<u32>,
    #[prost(bool, optional, tag = "4")]
    pub filenames_encrypted: Option<bool>,
    #[prost(uint64, optional, tag = "5")]
    pub cb_disk_original: Option<u64>,
    #[prost(uint64, optional, tag = "6")]
    pub cb_disk_compressed: Option<u64>,
    #[prost(uint32, optional, tag = "7")]
    pub unique_chunks: Option<u32>,
    #[prost(uint32, optional, tag = "8")]
    pub crc_encrypted: Option<u32>,
    #[prost(uint32, optional, tag = "9")]
    pub crc_clear: Option<u32>,
}

/// `ContentManifestSignature`.
#[derive(Clone, PartialEq, prost::Message)]
pub struct ManifestSignature {
    #[prost(bytes = "vec", optional, tag = "1")]
    pub signature: Option<Vec<u8>>,
}
