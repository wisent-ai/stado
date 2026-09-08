//! `stado bootstrap` implementation: provision the agent on remote boxes.
//!
//! Port of `stado/deploy/bootstrap.py`, cut over to the Rust release
//! binaries. For each kind=local registry entry with an ssh field,
//! downloads the platform-appropriate release binaries (stado +
//! stado-fix + stado-watchdog) from the public Stado release endpoint
//! ([`crate::config::stado_release_api_url`]) into
//! `~/.stado/bin/` on the remote host (platform picked by remote uname:
//! Linux x86_64 -> linux-amd64, Darwin arm64 -> darwin-arm64), writes a
//! systemd unit that runs `stado agent` and measures capacity from current
//! CPU, RAM, disk, and accelerator state (`WC_PYTHON` points at the host's
//! python3 — job payloads still run as Python), then enables it so the
//! agent comes back up on reboot. Targets with ssh=null are listed as
//! unprovisioned.
//!
//! Idempotent: re-running just refreshes the binaries, unit and
//! enablement. The existing capacity broadcast loop continues
//! uninterrupted because the unit's ExecStart is identical.

mod dispatch;
mod install;
mod provision;
mod units;

pub use dispatch::{empty_hf_fetcher, run_bootstrap};
pub use install::{install_spec, remote_install_script, ssh_argv, REMOTE_INSTALL_SCRIPT};
