//! Canonical navigation for experimental wire and session code.
//!
//! The established `experimental::*` modules remain public because their
//! paths appear in integration harnesses and provenance records. New code can
//! use this facade while the implementation is decomposed incrementally.

pub use crate::experimental::{
    actor_channel, actor_open, admission, archive, character_move, class_checksum, control,
    control_messages, control_receive, game_state_open, handshake, handshake_lab, manager_open,
    packet, packet_lab, pawn_open, pawn_restore, ping, probe_client, probe_initialise, quest_open,
    quest_table, rep_layout, rep_rpc, rep_text, selected_rpc_capture, selected_visibility_replay,
    sequencing, server_fps, server_query, session_churn, session_hold, session_open, steam_auth,
    tag_dictionary, tag_query, visibility, visibility_gather, visibility_state,
};
