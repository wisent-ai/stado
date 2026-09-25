//! The Wisent product catalog and product lifecycle, run as `stado product`.
//!
//! This crate was the separate `wisent-products` program until its capability
//! moved into Stado, which already owns the hosts, services and releases every
//! product lifecycle operation acts on. Stado is its only caller: it passes
//! the running build's identity to [`cli::run`], and every retained command
//! and compiler record names that build.

mod cargo;
pub mod catalog;
pub mod cli;
pub mod common;
mod creation;
mod documentation;
mod install;
mod native;
mod paths;
mod registry;
mod schedule;
mod signing;
mod source;
mod state;

use std::sync::OnceLock;

/// The Stado build executing these operations.
#[derive(Clone, Copy, Debug)]
pub struct Build {
    /// The Stado package version.
    pub version: &'static str,
    /// The Stado source revision the build embeds, `-dirty` when measured so.
    pub source_revision: &'static str,
    /// The Skarbiec item holding the fleet's Apple certificate and key, used
    /// when neither `WISENT_CODESIGN_CREDENTIAL_ITEM` nor
    /// `WISENT_CODESIGN_CERTIFICATE_PEM` names another credential. A machine
    /// running Stado keeps no signing identity of its own.
    pub signing_item: &'static str,
}

static BUILD: OnceLock<Build> = OnceLock::new();

/// The build [`cli::run`] was given. Records written before that call, which
/// only this crate's own tests could make, say `unknown`.
pub(crate) fn build() -> Build {
    BUILD.get().copied().unwrap_or(Build {
        version: "unknown",
        source_revision: "unknown",
        signing_item: "",
    })
}
