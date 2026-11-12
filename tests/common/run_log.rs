//! Shared run-log privacy list: words that only authored world declarations
//! and state use, so none may appear in the server's or the join probe's JSON
//! output. Test support only; included with `#[path]` under `#[cfg(test)]`.
#![allow(dead_code)]

/// Words no run log line may contain.
pub const RUN_LOG_FORBIDDEN: [&str; 5] = [
    "IndependentBox",
    "authored",
    "catalog_id",
    "rotation",
    "placement",
];
