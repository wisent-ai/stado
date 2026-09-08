//! The `stado storage` command surface: the subcommand set, the dispatch that
//! routes it, and the locator flags the cross-store commands share.

pub(in crate::cli::storage) mod commands;
pub(in crate::cli::storage) mod endpoint;
