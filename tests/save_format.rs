//! In-process authored save-format and preservation tests.

// Shared helpers retain their crate-root type bindings.
#[path = "common/c001_scalar_owners.rs"]
mod scalar;

#[path = "save_format/archive.rs"]
mod archive;

#[path = "save_format/archive_frame.rs"]
mod archive_frame;

#[path = "save_format/c001_inventory_source.rs"]
mod c001_inventory_source;

#[path = "save_format/c001_variables.rs"]
mod c001_variables;

#[path = "save_format/component.rs"]
mod component;

#[path = "save_format/int_array.rs"]
mod int_array;

#[path = "save_format/modifier_lifecycle.rs"]
mod modifier_lifecycle;

#[path = "save_format/modifier_restore.rs"]
mod modifier_restore;

#[path = "save_format/modifier_timing.rs"]
mod modifier_timing;

#[path = "save_format/preservation.rs"]
mod preservation;

#[path = "save_format/preservation_containers.rs"]
mod preservation_containers;

#[path = "save_format/preservation_fuzz.rs"]
mod preservation_fuzz;

#[path = "save_format/preservation_streaming.rs"]
mod preservation_streaming;

#[path = "save_format/property_tree.rs"]
mod property_tree;

#[path = "save_format/prospect_components.rs"]
mod prospect_components;

#[path = "save_format/recorders.rs"]
mod recorders;

#[path = "save_format/restore_rules.rs"]
mod restore_rules;

#[path = "save_format/struct_array.rs"]
mod struct_array;

#[path = "save_format/tagged.rs"]
mod tagged;

#[path = "save_format/typed_value.rs"]
mod typed_value;
