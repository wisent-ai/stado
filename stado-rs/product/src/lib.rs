//! The Wisent product catalog and product lifecycle, run as `stado product`.
//!
//! This crate was the separate `wisent-products` program until its capability
//! moved into Stado, which already owns the hosts, services and releases every
//! product lifecycle operation acts on. Stado is its only caller: it passes
//! the running build's identity to [`cli::run`], and every retained command
//! and compiler record names that build.

mod build_trees;
pub mod callers;
mod cargo;
pub mod catalog;
pub mod changelog;
pub mod cli;
pub mod common;
pub mod compiler_cache;
mod creation;
mod documentation;
mod install;
mod native;
pub mod node_runtime;
mod paths;
mod registry;
mod release_steps;
mod signing;
mod source;
mod state;
mod surface;
pub mod swift_cache;

/// The committed tree of a revision as plain files, the tree an install
/// builds; `stado quality check` reads its gates there too.
pub use source::export as export_committed_source;

/// Export the locked private Git crates for a credential-free release worker.
pub use cargo::private_sources::{
    export as export_private_cargo_sources, INPUT_NAME as PRIVATE_CARGO_INPUT_NAME,
};

use std::sync::OnceLock;

/// The Stado build executing these operations.
#[derive(Clone, Copy, Debug)]
pub struct Build {
    /// The Stado package version.
    pub version: &'static str,
    /// The Stado source revision the build embeds, `-dirty` when measured so.
    pub source_revision: &'static str,
    /// The role whose item holds the fleet's Apple certificate and key — the
    /// item carrying `stado:role:<role>` — used when neither
    /// `WISENT_CODESIGN_ROLE` nor `WISENT_CODESIGN_CERTIFICATE_PEM` names other
    /// material. A machine running Stado keeps no signing identity of its own,
    /// and no item id is named: the vault says which item plays the role.
    pub signing_role: &'static str,
}

static BUILD: OnceLock<Build> = OnceLock::new();

/// The build [`cli::run`] was given. Records written before that call, which
/// only this crate's own tests could make, say `unknown`.
pub(crate) fn build() -> Build {
    BUILD.get().copied().unwrap_or(Build {
        version: "unknown",
        source_revision: "unknown",
        signing_role: "",
    })
}
