//! One file per transition an operation can be driven through: `apply`,
//! `restore`, `verify`, and the `failure` writes all three share. Each entry
//! point here expects the journal lock to be held already, so the lock
//! lifetime stays with the caller in `entry` and never with the transition.
//!
//! `model` is re-anchored here so `verify` keeps the `super::model::…` paths
//! it was written with.

use crate::cli::resources::model;

mod apply;
mod failure;
mod restore;
mod verify;

pub(in crate::cli::resources::engine) use apply::execute_locked;
pub(in crate::cli::resources::engine) use restore::restore_locked;
pub(in crate::cli::resources::engine) use verify::verify_locked;
