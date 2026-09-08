//! Bootstrap stage one: put the release binaries on the remote host, retire
//! the agents this provision supersedes, and read back the platform, the
//! job-runtime Python path and the installed Stado path.

mod parse;
mod retire;
mod script;
mod specs;

pub use script::{remote_install_script, REMOTE_INSTALL_SCRIPT};
pub use specs::{install_spec, ssh_argv};

pub(super) use parse::parse_remote_install;
pub(super) use retire::retire_superseded_agent_units_spec;
pub(super) use script::{WC_BIN_DEFAULT, WC_PYTHON_DEFAULT};
pub(super) use specs::installed_spec;
