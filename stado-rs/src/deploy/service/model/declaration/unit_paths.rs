//! Native init-system unit locations. These directory names are identities
//! imposed by launchd and systemd, not configurable Stado policy.

use std::path::{Path, PathBuf};

pub const LAUNCHD_DAEMONS: &str = "/Library/LaunchDaemons";
pub const LAUNCHD_AGENTS: &str = "/Library/LaunchAgents";
pub const USER_LAUNCHD_AGENTS: &str = "Library/LaunchAgents";
pub const SYSTEMD_UNITS: &str = "/etc/systemd/system";
pub const USER_SYSTEMD_UNITS: &str = ".config/systemd/user";

/// All native spellings of the resident product unit on this host.
pub fn resident_unit_paths(home: &Path, label: &str) -> impl Iterator<Item = PathBuf> {
    [
        home.join(USER_LAUNCHD_AGENTS)
            .join(format!("{label}.plist")),
        Path::new(LAUNCHD_DAEMONS).join(format!("{label}.plist")),
        home.join(USER_SYSTEMD_UNITS)
            .join(format!("{label}.service")),
        Path::new(SYSTEMD_UNITS).join(format!("{label}.service")),
    ]
    .into_iter()
}
