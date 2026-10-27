//! Weles recordings cleanup: whole-run eviction of every run directory under
//! the host's declared recordings root (`targets[].weles.recordings_dir`); a
//! host that declares none is skipped as `recordings_dir_undeclared`.
//!
//! The scan is path-based: the safety gate is the ordered series of refusals
//! before the removal (hidden or reserved names, symbolic links, another
//! owner or device) plus the lexical check that a run directory is a direct
//! child of the root. Under the disk-full rule a recording goes whether or
//! not it was uploaded.
//!
//! Layout: `tree_ops` holds the sizing and removal walks the clone cleaner
//! shares, and `scan` the refusal series and the eviction itself.

mod scan;
mod tree_ops;

pub use scan::{scan_weles, CLEANER};

pub(super) use tree_ops::{dir_size, remove_tree};
