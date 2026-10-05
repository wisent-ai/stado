//! `stado bootstrap` implementation: put Stado on remote boxes.
//!
//! For each kind=local registry entry with an ssh field, installs the exact
//! signed `stado` release from the public Stado release endpoint
//! ([`crate::config::stado_release_api_url`]) into `~/.stado/bin/` on the
//! remote host (platform picked by remote uname), provisions the host's
//! workload-agent grant, retires any unit an earlier bootstrap left running
//! the removed standalone queue agent, and then runs `stado bootstrap
//! --local --target <name>` there, so the host's own installer writes the one
//! `com.wisent.stado` unit running `stado serve` with every role the registry
//! declares. Targets with ssh=null are listed as unprovisioned.
//!
//! Idempotent: re-running refreshes the binary and the host unit.

mod dispatch;
mod install;
mod provision;
mod units;

pub use dispatch::{empty_hf_fetcher, one_process_refusal, run_bootstrap};
pub use install::{install_spec, remote_install_script, ssh_argv, REMOTE_INSTALL_SCRIPT};
