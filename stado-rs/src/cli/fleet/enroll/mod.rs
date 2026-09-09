//! Machine-initiated enrollment: `join`, `pending`, `approve`, `reject`.
//!
//! The machine being added knows everything about itself, so the flow
//! starts there: `stado fleet join` announces the machine's real hostname,
//! OS and architecture as an `enrollments/<hostname>.json` request in the
//! configured store (and prints it, for setups where the store is not
//! shared and the request travels by any channel). The operator lists
//! requests with `pending` and turns one into a registered target with
//! `approve` — the same validated compare-and-swap registry write as every
//! other fleet command, so a colliding host identity is refused by the
//! registry-v2 contract, never papered over.
//!
//! Both `join` and `approve` honor the fleet's central enrollment catalog
//! (`registry.enrollment`, see [`catalog`]): a path the catalog disables
//! is refused.

pub mod catalog;
pub mod legacy;

mod commands;
mod request;

pub use commands::{approve, join, pending, reject};
pub use request::{
    build_invited_request, build_request, pending_request, release_platform, request_destination,
    request_invite_id, request_target_name, target_name_for,
};
