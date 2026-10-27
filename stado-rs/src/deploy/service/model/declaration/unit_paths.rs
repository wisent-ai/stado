//! Native init-system unit locations. These directory names are identities
//! imposed by launchd and systemd, not configurable Stado policy.

use std::path::{Path, PathBuf};

// Each home-relative directory is spelled once, here, so the bare constant a
// path is joined onto and the `$HOME` shell word a walk expands are one name.
macro_rules! user_launchd_agents {
    () => {
        "Library/LaunchAgents"
    };
}
macro_rules! user_systemd_units {
    () => {
        ".config/systemd/user"
    };
}

pub const LAUNCHD_DAEMONS: &str = "/Library/LaunchDaemons";
pub const LAUNCHD_AGENTS: &str = concat!("/", user_launchd_agents!());
pub const USER_LAUNCHD_AGENTS: &str = user_launchd_agents!();
pub const SYSTEMD_UNITS: &str = "/etc/systemd/system";
pub const USER_SYSTEMD_UNITS: &str = user_systemd_units!();

/// The directories this fleet installs launchd units into, in the order
/// [`crate::deploy::service::LOADED_UNITS_SCRIPT`] walks them, each a shell
/// word whose `$HOME` expands on the host it runs on. A unit one enumeration
/// can see and another cannot is how a label ends up in nobody's set, so
/// every walk takes this one list.
pub const LAUNCHD_UNIT_DIRECTORIES: &[&str] = &[
    LAUNCHD_DAEMONS,
    concat!("$HOME/", user_launchd_agents!()),
    LAUNCHD_AGENTS,
];

/// The directories this fleet writes systemd units into, system units and
/// then the user's, as shell words like [`LAUNCHD_UNIT_DIRECTORIES`].
pub const SYSTEMD_UNIT_DIRECTORIES: &[&str] =
    &[SYSTEMD_UNITS, concat!("$HOME/", user_systemd_units!())];

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

/// A shell program with this module's directories spelled into it: every
/// `@LAUNCHD_DAEMONS@`, `@USER_LAUNCHD_AGENTS@`, `@SYSTEMD_UNITS@` and
/// `@USER_SYSTEMD_UNITS@` becomes the declared directory, so a script text
/// holds no copy of its own. The two user directories are home-relative, so
/// the script writes `$HOME/` before them.
pub fn with_unit_directories(script: &str) -> String {
    script
        .replace("@LAUNCHD_DAEMONS@", LAUNCHD_DAEMONS)
        .replace("@USER_LAUNCHD_AGENTS@", USER_LAUNCHD_AGENTS)
        .replace("@SYSTEMD_UNITS@", SYSTEMD_UNITS)
        .replace("@USER_SYSTEMD_UNITS@", USER_SYSTEMD_UNITS)
}
