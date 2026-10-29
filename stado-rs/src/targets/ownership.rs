//! Who owns a host's files beyond Stado's own tree.
//!
//! A fleet host is either dedicated to the fleet, where every rebuildable
//! cache and snapshot on it is the fleet's to reclaim, or shared: a person's
//! own computer that also runs fleet work, where the caches, project build
//! trees and Time Machine snapshots in that person's home are theirs. The
//! registry entry says which (`targets[].ownership`), and the janitor on a
//! shared host deletes only inside Stado's own tree and reports the rest.

use serde_json::Value;

use super::ComputeTarget;

/// The registry key of a target's ownership.
pub const OWNERSHIP_KEY: &str = "ownership";
/// A host given to the fleet; the declaration's absence means this.
pub const OWNERSHIP_DEDICATED: &str = "dedicated";
/// A person's own computer that also runs fleet work.
pub const OWNERSHIP_SHARED: &str = "shared";

/// A host's declared ownership.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOwnership {
    Dedicated,
    Shared,
}

impl HostOwnership {
    /// The word the registry stores.
    pub fn word(self) -> &'static str {
        match self {
            HostOwnership::Dedicated => OWNERSHIP_DEDICATED,
            HostOwnership::Shared => OWNERSHIP_SHARED,
        }
    }
}

impl ComputeTarget {
    /// The ownership this target's registry entry declares, dedicated when
    /// it declares none; a value that is neither word is refused by name.
    pub fn ownership(&self) -> Result<HostOwnership, String> {
        match self.extra.get(OWNERSHIP_KEY) {
            None | Some(Value::Null) => Ok(HostOwnership::Dedicated),
            Some(Value::String(word)) if word == OWNERSHIP_DEDICATED => {
                Ok(HostOwnership::Dedicated)
            }
            Some(Value::String(word)) if word == OWNERSHIP_SHARED => Ok(HostOwnership::Shared),
            Some(other) => Err(format!(
                "targets[{}].{OWNERSHIP_KEY} is {other}; declare {OWNERSHIP_DEDICATED:?} or \
                 {OWNERSHIP_SHARED:?}",
                self.name
            )),
        }
    }
}
