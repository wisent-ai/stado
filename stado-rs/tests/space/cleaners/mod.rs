//! Cleaner writes and bounded cleanup through the real product binary.

use std::fs;

use serde_json::Value;

use crate::fixture::Host;

mod bounded;
mod declaring;

/// The canonical registry document as it stands on disk right now.
pub(super) fn registry(host: &Host) -> Value {
    serde_json::from_slice(&fs::read(host.storage.join("registry.json")).unwrap()).unwrap()
}
