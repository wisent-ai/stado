//! The two things every case in this area shares: the isolated fleet that
//! names this machine, and the guard that owns a real launchd label for the
//! length of one case.
//!
//! Keeping them here is what keeps them singular. The suite this replaces had
//! its host, its init system and its cleanup re-invented per case — as a
//! script named `ssh` on PATH, a directory of stand-in `launchctl`, `plutil`,
//! `stat`, `id`, `sudo` and `journalctl` executables, and a call log read
//! instead of a system. There is one fleet and one unit guard now, both real,
//! and no case can reach past them to a substituted tool.

pub mod fleet;
pub mod unit;
