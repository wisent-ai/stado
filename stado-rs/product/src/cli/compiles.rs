//! Which parsed `stado product` operations compile into a build directory the
//! disk janitor may reclaim, declared beside the operations themselves so
//! Stado's caller holds the janitor's cleanup lock for exactly those.
//!
//! An installation, an update, a Cargo build and every Swift operation
//! compile, resolve or restore into the package's build tree. Reading retained
//! Cargo results does not. The editor
//! server only answers from settings an index already wrote. Holding the
//! cleanup lock for its lifetime would prevent the janitor and installations
//! from ever acquiring their turn.

use clap::ArgMatches;

/// Whether `matches` (the `stado product` invocation) compiles into the
/// reclaimable build tree.
pub fn compiles(matches: &ArgMatches) -> bool {
    match matches.subcommand() {
        Some(("cargo", cargo)) => !matches!(
            cargo.get_one::<String>("operation"),
            Some(operation) if operation == "results"
        ),
        Some(("install" | "update" | "swift", _)) => true,
        _ => false,
    }
}
