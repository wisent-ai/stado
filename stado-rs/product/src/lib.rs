//! The Wisent product catalog and product lifecycle, run as `stado product`.
//!
//! This crate was the separate `wisent-products` program until its capability
//! moved into Stado, which already owns the hosts, services and releases every
//! product lifecycle operation acts on. Stado is its only caller: it passes
//! the running build's identity to [`cli::run`], and every retained command
//! and compiler record names that build.

mod advisories;
mod build_trees;
pub mod callers;
mod cargo;
pub use cargo::findings;
pub mod cargo_lock;
pub mod catalog;
pub mod changelog;
pub mod cli;
pub mod common;
pub mod compiler_cache;
mod creation;
mod dependencies;
mod desktop_app;
mod documentation;
mod install;
mod native;
pub mod node_runtime;
mod paths;
mod registry;
mod release_steps;
pub mod serve_stops;
mod signing;
mod source;
mod state;
mod surface;
pub mod swift_cache;
pub mod word;

pub use install::plan::conflict::InstallationHostConflict;
/// An update policy the operator wrote that cannot be followed.
pub use install::policy::InvalidPolicy;
/// A source that cannot be installed as asked: no canonical checkout for
/// the repository, or a commit no canonical branch carries.
pub use source::{MissingCheckout, NotCanonical};

/// The one canonical checkout of a repository (`owner/name`) in the
/// runtime's workspace, validated as one: refused with [`MissingCheckout`]
/// when the workspace holds none. `stado quality hooks` finds a product's
/// checkouts with it.
pub use source::checkout as canonical_checkout;

/// Declare again the lasting state an installation's recipe declares through
/// Stado, for a service its install path did not declare it for, and whether
/// a recipe declares any.
pub use install::{declares as installation_declares, redeclare as redeclare_installation};

/// The committed tree of a revision as plain files, the tree an install
/// builds; `stado quality check` reads its gates there too.
pub use source::export as export_committed_source;

/// Export the locked private Git crates for a credential-free release worker.
pub use cargo::private_sources::{
    export as export_private_cargo_sources, INPUT_NAME as PRIVATE_CARGO_INPUT_NAME,
};

/// The code-signing report of one file, as `stado product install` judges a
/// program it places, and whether that report is one macOS keeps a single
/// identity for across builds (a Developer ID or Apple Development
/// signature, or no native code at all). An ad-hoc signature is not: every
/// build is a new program to macOS, and each one asks every privacy
/// question again.
pub use signing::{acceptable as signature_acceptable, inspect as inspect_signature};

/// Signing one program for a host other than this one (`stado host
/// upgrade`): the choice of keychain and identity a command line made, the
/// team its signature will be anchored to (read before anything is signed),
/// the stable code identifier a product's program carries, and the signature
/// itself.
pub use signing::{
    identifier as code_identifier, sign_with as sign_program, Choice as SigningChoice,
    Team as SigningTeam,
};

use std::sync::OnceLock;

/// The Stado build executing these operations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
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
    /// The catalog id of the program executing these operations: an
    /// installation of this product hands its post-build steps to the build
    /// it made (`stado product finish`).
    pub product: &'static str,
}

/// The role whose item holds the fleet's Apple certificate and key: the vault
/// item carrying `stado:role:macos-development-signing`. It signs every product
/// installation unless the operator names another role through
/// `WISENT_CODESIGN_ROLE` or supplies `WISENT_CODESIGN_CERTIFICATE_PEM`.
///
/// A role says what the secret is for, never which item holds it, so renaming
/// or replacing the item changes nothing here; it is the same role the
/// release manifests name in `secret_env`.
pub const SIGNING_ROLE: &str = "macos-development-signing";

static BUILD: OnceLock<Build> = OnceLock::new();

/// Declare the compiled Stado identity before any product operation records
/// an external command. A second declaration must name the same build.
pub fn register_build(build: Build) {
    let recorded = BUILD.get_or_init(|| build);
    assert_eq!(
        *recorded, build,
        "product build identity changed within one process"
    );
}

/// The Stado build registered at process entry.
pub(crate) fn build() -> Build {
    *BUILD
        .get()
        .expect("product build identity must be registered before product operations")
}
