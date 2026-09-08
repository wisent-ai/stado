//! `stado placement move` — one fenced transaction for a colocated service
//! group — and `stado route placement publish`, which makes the registry the
//! only writer of a host's Weles placement policy.
//!
//! The registry profile is the complete operational contract: concrete units
//! per host, stop/start order, durable files, loopback health probes, and routing
//! units. The command claims the profile through registry CAS, fences the source,
//! copies state only after writers stop, activates and probes the destination,
//! then commits the service declarations with a second CAS. Every failure before
//! that commit restores destination files, routing, and source services.
//!
//! Both halves answer the same question — which host may do what — and the
//! second half exists because one part of that question was answered twice. A
//! service's placement lives in the registry and moves under transaction; a
//! worker's placement lived in the registry AND in a file on the worker's own
//! disk, and only the file decided.

mod candidates;
mod commands;
mod policy;
mod transfer;

pub use commands::{dispatch, PlacementCommands};
pub(crate) use policy::{
    normalize_hostname, policy_document, policy_effect, publish_placement_policy_report,
    RECONCILED_BY,
};
