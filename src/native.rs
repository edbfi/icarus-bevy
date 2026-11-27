//! Canonical navigation for the native authored-world and player domains.
//!
//! The leaf modules keep their established `native_*` paths for callers and
//! evidence references. This facade provides domain-oriented paths while the
//! implementation is migrated in smaller, behavior-preserving steps.

/// Authored world state, validation, and runtime adapters.
pub mod world {
    pub use crate::native_inventory as inventory;
    pub use crate::native_inventory_restore as inventory_restore;
    pub use crate::native_lifecycle as lifecycle;
    pub use crate::native_lifecycle_runtime as lifecycle_runtime;
    pub use crate::native_modifiers as modifiers;
    pub use crate::native_relationships as relationships;
    pub use crate::native_runtime as runtime;
    pub use crate::native_scalar_owners as scalar_owners;
    pub use crate::native_world as catalog;
}

/// Explicit C001 fixture adapters and their authored import contracts.
pub mod c001 {
    pub use crate::native_c001_inventory_owners as inventory_owners;
    pub use crate::native_c001_owners as owners;
    pub use crate::native_c001_scalar_owners as scalar_owners;
    pub use crate::native_import as import;
}

/// Native player state and player-facing runtime helpers.
pub mod players {
    pub use crate::leave_runtime_proof;
    pub use crate::native_player_inventory as inventory;
    pub use crate::native_player_runtime as runtime;
    pub use crate::native_players as registry;
    pub use crate::player_inventory_projection as inventory_projection;
    pub use crate::spawn_placement;
}

/// Native replication and hosting adapters.
pub use crate::native_hosting as hosting;
pub use crate::native_replication as replication;
