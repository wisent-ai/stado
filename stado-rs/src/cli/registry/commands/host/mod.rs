//! `stado registry host show|add|edit|remove|path ...` — onboard a machine,
//! change or retire it, and manage the ordered SSH connection paths it is
//! reachable on.

pub(in crate::cli::registry) mod add;
pub(in crate::cli::registry) mod edit;
pub(in crate::cli::registry) mod path;
pub(in crate::cli::registry) mod remove;
pub(in crate::cli::registry) mod show;
