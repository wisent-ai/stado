//! The `clap` declaration of the whole `stado` command tree.
//!
//! [`Cli`] is the root parser and `Commands` the one subcommand enum it
//! carries, assembled with `#[command(flatten)]` from the four declaration
//! blocks of [`root`]. `clap` inserts a flattened enum's variants at the
//! position of the variant that flattens it, so this tree accepts exactly the
//! command lines — and prints exactly the help — that one undivided enum did.
//!
//! [`fleet`], [`jobs`] and [`spend`] hold the nested subcommand enums those
//! blocks name.

use clap::{Parser, Subcommand};

pub mod fleet;
pub mod jobs;
pub mod root;
pub mod spend;

#[derive(Parser)]
#[command(
    // Not bare `version`: that prints CARGO_PKG_VERSION alone, and one
    // version has named several different trees of this crate. `--version`
    // is where an operator asks which build a host is running, so it answers
    // with the revision too.
    version = crate::build_identity::BUILD_IDENTITY,
    about = "Stado — policy-controlled queue and compute control plane."
)]
pub struct Cli {
    #[command(subcommand)]
    pub(crate) command: Option<Commands>,
}

/// The four declaration blocks of `stado --help`, in the order they are
/// printed. Flattening keeps one flat command tree: `stado submit` stays
/// `stado submit`, and no group name appears on a command line.
#[derive(Subcommand)]
pub(crate) enum Commands {
    #[command(flatten)]
    Installation(root::installation::InstallationCommands),
    #[command(flatten)]
    Work(root::work::WorkCommands),
    #[command(flatten)]
    Planes(root::planes::PlaneCommands),
    #[command(flatten)]
    Platform(root::platform::PlatformCommands),
}
