//! The `BoxProvider` adapter: the lifecycle half of the Box provider.
//!
//! `states` holds the Box state sets and the fixed machine shape list,
//! `provider` the struct with its construction, admission, preflight and
//! create / renew / release verbs, and `instances` the generic `Provider`
//! implementation that the scheduler and the CLI drive.

mod instances;
mod provider;
mod states;

/// Named out of tree as `crate::providers::r#box::BoxProvider`: re-exported
/// again by `crate::providers`, constructed by `providers::get_provider`
/// and `coordinator::passes::providers`, and taken by reference by the
/// `scheduler::dispatch::box` admit, session and reconcile passes.
pub use provider::BoxProvider;
