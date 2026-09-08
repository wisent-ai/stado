//! The slot and what it is made of: the live [`ActiveSlot`] with its process
//! group, log handle and janitor hold; the queue-owned work directory it
//! executes in; and the Python-parity conversions every record here is
//! formatted with.
//!
//! `disk_cleanup` is imported here rather than in the parts because the
//! janitor paths the slot holds are named `super::disk_cleanup::...` exactly
//! as they were when this was one file.

use crate::providers::local::disk_cleanup;

use super::*;

mod slot;
mod text;
mod workdir;

pub use slot::*;
pub use text::*;
// Every item here is crate-visible, not public: matching the re-export to
// what the part actually holds keeps the glob meaningful.
pub(crate) use workdir::*;
