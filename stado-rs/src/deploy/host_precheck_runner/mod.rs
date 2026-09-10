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
//! | [`scope`] | organization or repository registration, and who may ask |
//! | [`github`] | the GitHub credential and every call made with it |
//! | [`credentials`] | Skarbiec reads, including the routed Probierz identity |
//! | [`brama`] | the Brama a runner dials and the Skarbiec beside it |
//! | [`installer`] | the exact installer program one registration renders |
//! | [`install`] | installing a runner and everything its profile declares |
//! | [`lifecycle`] | restarting, repairing and removing an installed runner |
//! | [`status`] | what one runner, one host, or the fleet reports |
//! | [`diagnostics`] | what a runner that will not start is saying, read whole |
//! | [`report`] | the shape of an answer and the fields read out of a program |
//! | [`model_review`] | the model-review bearer a repository's CI presents |
//! | [`publisher`] | a desktop publisher repository's release secrets |
//! | [`apple_signing`], [`developer_id`] | the Developer ID bundle |
//! | `linux_install`, `linux_scripts`, `macos_install`, `macos_runtime`, `macos_scripts` | the programs the host runs |

mod apple_signing;
mod brama;
mod credentials;
mod declaration;
mod developer_id;
mod diagnostics;
mod github;
mod install;
mod installer;
mod lifecycle;
mod linux_install;
mod linux_scripts;
mod macos_install;
mod macos_runtime;
mod macos_scripts;
mod model_review;
mod platform;
mod publisher;
mod report;
mod scope;
mod status;

// `brama`, `credentials` and `report` carry no name a caller outside this
// module uses; their submodules address each other directly.
pub use self::declaration::*;
pub use self::developer_id::*;
pub use self::diagnostics::*;
pub use self::github::*;
pub use self::install::*;
pub use self::installer::*;
pub use self::lifecycle::*;
pub use self::model_review::*;
pub use self::platform::*;
pub use self::publisher::*;
pub use self::scope::*;
pub use self::status::*;
