//! The long-running host processes: every resident Stado role — the worker,
//! the scheduling tick, the API listener, release reconciliation, health
//! publication — runs as a role of `stado serve`, and the host's node on
//! Stado's own network runs beside it under `stado network`, so a host whose
//! `serve` is broken stays reachable. The third block of `stado --help`.

use clap::Subcommand;

/// The third block of `stado` verbs. Flattened into
/// `super::super::Commands`, so splitting the declaration across files
/// changes no command line.
#[derive(Subcommand)]
pub(crate) enum PlaneCommands {
    /// Run this host's Stado components in one supervised process.
    Serve(Box<crate::cli::integrations::runtime::ServeArgs>),
    /// Stado's own host network: join it, read what it reaches, leave it.
    #[command(subcommand)]
    Network(crate::cli::network::NetworkCommands),
}
