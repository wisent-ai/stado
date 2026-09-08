//! `stado registry host add|path ...` — onboard a machine, and manage the
//! ordered SSH connection paths it is reachable on.

pub(in crate::cli::registry) mod add;
pub(in crate::cli::registry) mod path;
