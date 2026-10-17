//! The Boat provider adapter for Boat sandboxes (Boat Public API v1).
//!
//! The provider is a lifecycle adapter: admission goes through
//! `targets::box_capabilities`, capacity is preflighted against the account
//! limits endpoint, the sandbox's auto-stop renews via PATCH, and release is
//! stop-or-delete per `boat.release_mode`. A deletion of a sandbox a
//! running/ job still references bridges through the fenced cancel path
//! (`scheduler::dispatch::box::cancel_box_for_legacy_move`).

mod adapter;
pub mod client;
pub mod http;
pub mod types;

/// Named out of tree as `crate::providers::r#box::BoxProvider`: re-exported
/// again by `crate::providers`, constructed by `providers::get_provider`
/// and `coordinator::passes::providers`, and taken by reference by the
/// `scheduler::dispatch::box` admit, session and reconcile passes. It is
/// also what the `super::BoxProvider::from_env` doc links in the `http`
/// sibling resolve against.
pub use adapter::{BoxProvider, ReleaseMode};
pub use client::BoxClient;
pub use client::{FileUrl, TtlUpdate};
pub use http::BoxHttpTransport;
pub use types::{
    BoxApiError, BoxCommandResult, BoxError, BoxEventPage, BoxInfo, BoxLimits, BoxPromptRun,
    BoxState, PromptStatus,
};
