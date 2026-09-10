//! Rust-owned lifecycle for GitHub runner profiles declared by the fleet.
//!
//! Stado selects a profile from the compiled declaration, resolves the host
//! from the canonical registry, obtains a short-lived GitHub registration token
//! through Skarbiec, and sends one fixed installer program over the audited host
//! channel. No Python helper or operator shell is part of the lifecycle.
//!
//! The lifecycle is one module split by responsibility, and this file is its
//! public face: every name a caller or a test used before the split still
//! resolves at `crate::deploy::host_precheck_runner`.
//!
//! | submodule | what it owns |
//! |---|---|
//! | [`declaration`] | the compiled profile declaration and the host it resolves |
//! | [`platform`] | the two platforms, template substitution, the host job gate |
//! | [`lifecycle`] | restarting, repairing and removing an installed runner |
//! | [`accounts`] | every identity a runner presents: GitHub, Skarbiec, Brama, model review |
//! | [`release`] | registering a runner: the rendered installer and what one install declares |
//! | [`signing`] | the Developer ID bundle a desktop publisher signs with |
//! | [`verdict`] | what a runner, a host or the fleet reports, and how an answer is read |
//! | [`macos`], [`linux`] | the programs each platform's host runs |

mod accounts;
mod declaration;
mod lifecycle;
mod linux;
mod macos;
mod platform;
mod release;
mod signing;
mod verdict;

// `accounts::brama`, `accounts::credentials` and `verdict::report` carry no
// name a caller outside this module uses; their submodules address each
// other directly.
pub use self::accounts::*;
pub use self::declaration::*;
pub use self::lifecycle::*;
pub use self::platform::*;
pub use self::release::*;
pub use self::signing::*;
pub use self::verdict::*;
