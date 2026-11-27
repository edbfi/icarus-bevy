//! Canonical navigation for experimental save readers and preservation code.
//!
//! `experimental::*` remains the compatibility facade. The shared archive
//! primitive is available here as well as through [`crate::protocol`].

pub use crate::experimental::{
    archive, component, int_array, inventory_restore, inventory_weight, modifier_lifecycle,
    modifier_restore, modifier_timing, property_tree, prospect_info, recorders, restore_rules,
    struct_array, tagged, temperature, time_of_day, typed_value,
};
