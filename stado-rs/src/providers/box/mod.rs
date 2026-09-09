//! Box by ASCII provider adapter for fixed-shape Linux sandboxes.
//!
//! Port of `stado/providers/box/__init__.py`. The provider is a lifecycle
//! adapter: admission goes through `targets::box_capabilities`, capacity is
//! preflighted against the account limits endpoint, TTL renews via PATCH,
//! and release is stop-or-delete per `BOX_RELEASE_MODE`. Legacy
//! `delete_instance` calls on a box still referenced by a running/ job
//! bridge through the fenced cancel path
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
pub use adapter::BoxProvider;
pub use client::BoxClient;
pub use client::TtlUpdate;
pub use http::BoxHttpTransport;
pub use types::{
    BoxApiError, BoxCommandResult, BoxError, BoxEventPage, BoxInfo, BoxLimits, BoxPromptRun,
};
