//! Which parsed `stado product` operations compile into a build directory the
//! disk janitor may reclaim, declared beside the operations themselves so
//! Stado's caller holds the janitor's cleanup lock for exactly those.
//!
//! An installation, an update, a Cargo build and every Swift operation but
//! one compile, resolve or restore into the package's build tree. `swift
//! sourcekit` is a language server: it answers the editor from the compiler
//! settings an index already wrote and compiles nothing (its targets declare
//! `canCompile: false`). Held for its life, it kept the cleanup lock shared
//! for as long as an editor stayed open, so the janitor waited for its turn
//! without cleaning and every installation on the machine was refused while
//! it waited.

use clap::ArgMatches;

/// The Swift operation that serves an editor and builds nothing.
const LANGUAGE_SERVER: &str = "sourcekit";

/// Whether `matches` (the `stado product` invocation) compiles into the
/// reclaimable build tree.
pub fn compiles(matches: &ArgMatches) -> bool {
    match matches.subcommand() {
        Some(("install" | "update" | "cargo", _)) => true,
        Some(("swift", swift)) => swift
            .get_one::<String>("operation")
            .is_some_and(|operation| operation != LANGUAGE_SERVER),
        _ => false,
    }
}
