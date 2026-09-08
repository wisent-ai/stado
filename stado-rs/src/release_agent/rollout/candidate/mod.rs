//! One candidate from the registry's desired coordinates to a live process:
//! fetched, verified, installed, started, and resolved back to a binary.

pub(crate) mod binary;
pub(crate) mod fetch;
pub(crate) mod spawn;
pub(crate) mod stage;
