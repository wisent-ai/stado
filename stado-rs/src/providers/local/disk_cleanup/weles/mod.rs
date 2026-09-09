//! Weles recordings cleanup: whole-run eviction gated on age, durable
//! upload proof, and run inactivity.
//!
//! Port of the weles half of `stado/providers/local/disk/cleanup.py`
//! (`_weles_upload_proof_ok`, `_weles_run_active`, `_weles_dir_size`,
//! `_scan_weles`). Unlike the HF cleaner this pass is path-based (as in
//! the Python): the safety gate is the ordered series of refusals before
//! `rmtree`, plus the lexical commonpath check — every refusal below is
//! covered by the module test suite.
//!
//! Layout: `eligibility` holds the upload-proof and run-activity gates,
//! `tree_ops` the sizing and removal walks the clone cleaner shares, and
//! `scan` the refusal series and bounded eviction itself.

mod eligibility;
mod scan;
pub mod tree_ops;

pub use scan::scan_weles;

pub(super) use tree_ops::{dir_size, remove_tree};
