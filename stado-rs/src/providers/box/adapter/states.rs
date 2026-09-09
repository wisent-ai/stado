//! The Box state sets and the fixed machine shape list.
//!
//! Python `_ACTIVE_STATES`, `_RUNNING_STATES` and `_BOX_MACHINE_TYPES`,
//! read by the `provider` and `instances` components beside this file.

use std::collections::BTreeSet;
use std::sync::LazyLock;

/// Python `_ACTIVE_STATES`.
pub(super) fn active_states() -> &'static BTreeSet<&'static str> {
    static STATES: LazyLock<BTreeSet<&'static str>> = LazyLock::new(|| {
        BTreeSet::from([
            "init",
            "provisioning",
            "provisioned",
            "cloning",
            "ready",
            "idle",
            "running",
            "archiving",
        ])
    });
    &STATES
}

/// Python `_RUNNING_STATES`.
pub(super) fn running_states() -> &'static BTreeSet<&'static str> {
    static STATES: LazyLock<BTreeSet<&'static str>> =
        LazyLock::new(|| BTreeSet::from(["ready", "idle", "running"]));
    &STATES
}

/// Python `_BOX_MACHINE_TYPES`.
pub(super) const BOX_MACHINE_TYPES: [&str; 3] = ["", "box", "box-linux-4cpu-8gb"];
