//! In-process native world, runtime and storage tests.

// Shared helpers retain their crate-root type bindings.
#[path = "common/c001_inventory_owners.rs"]
mod inventory_fixture;
#[path = "common/c001_owners.rs"]
mod owner_fixture;
#[path = "common/c001_scalar_owners.rs"]
mod scalar;
#[path = "common/c001_inventory_source.rs"]
mod source;

#[path = "native_domain/foundation.rs"]
mod foundation;

#[path = "native_domain/native_authored_inventory.rs"]
mod native_authored_inventory;

#[path = "native_domain/native_inventory_transactions.rs"]
mod native_inventory_transactions;

#[path = "native_domain/native_c001_inventory_owner_storage.rs"]
mod native_c001_inventory_owner_storage;

#[path = "native_domain/native_c001_inventory_owners.rs"]
mod native_c001_inventory_owners;

#[path = "native_domain/native_c001_owner_runtime.rs"]
mod native_c001_owner_runtime;

#[path = "native_domain/native_c001_owner_storage.rs"]
mod native_c001_owner_storage;

#[path = "native_domain/native_c001_owners.rs"]
mod native_c001_owners;

#[path = "native_domain/native_c001_scalar_owner_storage.rs"]
mod native_c001_scalar_owner_storage;

#[path = "native_domain/native_c001_scalar_owners.rs"]
mod native_c001_scalar_owners;

#[path = "native_domain/native_import_storage.rs"]
mod native_import_storage;

#[path = "native_domain/native_inventory.rs"]
mod native_inventory;

#[path = "native_domain/native_inventory_restore.rs"]
mod native_inventory_restore;

#[path = "native_domain/native_inventory_runtime.rs"]
mod native_inventory_runtime;

#[path = "native_domain/native_lifecycle.rs"]
mod native_lifecycle;

#[path = "native_domain/native_lifecycle_runtime.rs"]
mod native_lifecycle_runtime;

#[path = "native_domain/native_lifecycle_storage.rs"]
mod native_lifecycle_storage;

#[path = "native_domain/native_modifier_runtime.rs"]
mod native_modifier_runtime;

#[path = "native_domain/native_modifier_storage.rs"]
mod native_modifier_storage;

#[path = "native_domain/native_modifiers.rs"]
mod native_modifiers;

#[path = "native_domain/native_relationship_runtime.rs"]
mod native_relationship_runtime;

#[path = "native_domain/native_relationship_storage.rs"]
mod native_relationship_storage;

#[path = "native_domain/native_relationships.rs"]
mod native_relationships;

#[path = "native_domain/native_scalar_owner_runtime.rs"]
mod native_scalar_owner_runtime;

#[path = "native_domain/native_scalar_owners.rs"]
mod native_scalar_owners;

#[path = "native_domain/native_world.rs"]
mod native_world;

#[path = "native_domain/native_world_storage.rs"]
mod native_world_storage;

#[path = "native_domain/persistence_fuzz.rs"]
mod persistence_fuzz;

#[path = "native_domain/native_players.rs"]
mod native_players;
