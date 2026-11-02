//! In-process protocol, session and offline platform tests.

#[path = "protocol/admission.rs"]
mod admission;

#[path = "protocol/checksum.rs"]
mod checksum;

#[path = "protocol/control.rs"]
mod control;

#[path = "protocol/control_messages.rs"]
mod control_messages;

#[path = "protocol/control_receive.rs"]
mod control_receive;

#[path = "protocol/handshake.rs"]
mod handshake;

#[path = "protocol/handshake_lab.rs"]
mod handshake_lab;

#[path = "protocol/network_fuzz.rs"]
mod network_fuzz;

#[path = "protocol/packet.rs"]
mod packet;

#[path = "protocol/packet_lab.rs"]
mod packet_lab;

#[path = "protocol/peer_lab.rs"]
mod peer_lab;

#[path = "protocol/peer_lab_fuzz.rs"]
mod peer_lab_fuzz;

#[path = "protocol/ping.rs"]
mod ping;

#[path = "protocol/probe_client.rs"]
mod probe_client;

#[path = "protocol/prospect_advert.rs"]
mod prospect_advert;

#[path = "protocol/release.rs"]
mod release;

#[path = "protocol/sequencing.rs"]
mod sequencing;

#[path = "protocol/server_query.rs"]
mod server_query;

#[path = "protocol/steam_auth.rs"]
mod steam_auth;

#[path = "protocol/steam_fetch.rs"]
mod steam_fetch;

#[path = "protocol/steam_presence.rs"]
mod steam_presence;

#[path = "protocol/steam_service.rs"]
mod steam_service;

#[path = "protocol/steam_session_auth.rs"]
mod steam_session_auth;

#[path = "protocol/tag_dictionary.rs"]
mod tag_dictionary;
