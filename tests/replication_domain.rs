//! In-process actor replication, gameplay encoding and capture tests.

#[path = "pak_writer/mod.rs"]
mod pak_writer;

#[path = "replication_domain/actor_channel.rs"]
mod actor_channel;

#[path = "replication_domain/actor_open.rs"]
mod actor_open;

#[path = "replication_domain/capture_decode.rs"]
mod capture_decode;

#[path = "replication_domain/class_checksum.rs"]
mod class_checksum;

#[path = "replication_domain/codec_fuzz.rs"]
mod codec_fuzz;

#[path = "replication_domain/controller_initialisation.rs"]
mod controller_initialisation;

#[path = "replication_domain/game_state_open.rs"]
mod game_state_open;

#[path = "replication_domain/inventory_restore.rs"]
mod inventory_restore;

#[path = "replication_domain/inventory_weight.rs"]
mod inventory_weight;

#[path = "replication_domain/manager_open.rs"]
mod manager_open;

#[path = "replication_domain/pawn_open.rs"]
mod pawn_open;

#[path = "replication_domain/pawn_restore.rs"]
mod pawn_restore;

#[path = "replication_domain/pawn_stats.rs"]
mod pawn_stats;

#[path = "replication_domain/quest_actor_open.rs"]
mod quest_actor_open;

#[path = "replication_domain/rep_hit_result.rs"]
mod rep_hit_result;

#[path = "replication_domain/rep_layout.rs"]
mod rep_layout;

#[path = "replication_domain/rep_movement.rs"]
mod rep_movement;

#[path = "replication_domain/rep_rpc.rs"]
mod rep_rpc;

#[path = "replication_domain/rep_strings.rs"]
mod rep_strings;

#[path = "replication_domain/rep_tags.rs"]
mod rep_tags;

#[path = "replication_domain/rep_text.rs"]
mod rep_text;

#[path = "replication_domain/replication.rs"]
mod replication;

#[path = "replication_domain/server_fps.rs"]
mod server_fps;

#[path = "replication_domain/session_open.rs"]
mod session_open;

#[path = "replication_domain/stat_restore.rs"]
mod stat_restore;

#[path = "replication_domain/temperature.rs"]
mod temperature;

#[path = "replication_domain/temperature_tick.rs"]
mod temperature_tick;

#[path = "replication_domain/time_of_day.rs"]
mod time_of_day;

#[path = "replication_domain/visibility.rs"]
mod visibility;

#[path = "replication_domain/visibility_state.rs"]
mod visibility_state;
