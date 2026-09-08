//! Declared service repair: one capability over the repair steps compiled into
//! the shipped service catalog.

mod args;
mod catalog;
mod commands;
mod steps;

pub(crate) use args::RepairArgs;
pub(crate) use commands::dispatch;
