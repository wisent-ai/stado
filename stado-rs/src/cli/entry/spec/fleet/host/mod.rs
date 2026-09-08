//! `stado host`: the operating-system verbs that run on a registry host.
//!
//! `HostCommands` carries the two declaration blocks of this tree with
//! `#[command(flatten)]`, in the order they were declared: [`state`] reads a
//! host and changes its machine-level state, [`runs`] works inside a
//! delivered run tree and on the host's Stado configuration. `clap` inserts a
//! flattened enum's variants at the position of the variant that flattens
//! them, so `stado host --help` lists exactly what one undivided enum listed.

use clap::Subcommand;

pub mod runs;
pub mod state;
pub mod users;

#[derive(Subcommand)]
pub(crate) enum HostCommands {
    #[command(flatten)]
    State(state::HostStateCommands),
    #[command(flatten)]
    Runs(runs::HostRunCommands),
}
