//! The long-running host process: every resident Stado role — the worker,
//! the scheduling tick, the API listener, release reconciliation, health
//! publication — runs as a role of `stado serve`. The third block of
//! `stado --help`.

use clap::Subcommand;

/// The third block of `stado` verbs. Flattened into
/// `super::super::Commands`, so splitting the declaration across files
/// changes no command line.
#[derive(Subcommand)]
pub(crate) enum PlaneCommands {
    /// Run this host's Stado components in one supervised process.
    Serve(Box<crate::cli::integrations::runtime::ServeArgs>),
}
