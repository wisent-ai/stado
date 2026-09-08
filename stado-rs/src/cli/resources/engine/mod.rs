//! Preflight-first, journalled resource plan execution and restore.
//!
//! `entry` holds the command entry points and is the only place that takes
//! the journal lock; behind it `phases` holds one transition per file.
//! `selection` decides which actions an invocation may touch at all, and
//! `report` writes the preview an operator reviews and the closing summary.

mod entry;
mod phases;
mod report;
mod selection;

pub(crate) use entry::execute_autonomous;
pub use entry::{apply_shutdown, kill_irrational, restore, verify};
