//! The disk measured against the declarations: what the host needs, what the
//! declared mechanisms reach, and what nothing reaches.
//!
//! This exists because of a reading that was true and useless. On 2026-09-09
//! `charless-mac-mini` held 282 MB free of 228 GB, `stado space report` printed
//! `99%` and `janitor: cap_reached`, and a delivery to a leased account died
//! with `No space left on device`. Every figure in that report was correct.
//! None of them answered the question an operator and an automat both have:
//! how far is this host from the free space it declares, can the declared
//! mechanisms get it there, and if not, what is holding the bytes.
//!
//! So the report measures four things it already had the parts for:
//!
//! - **the need**, from the declared watermarks the host is measured against;
//! - **the coverage**, from [`crate::deploy::host_reclaim::declared_stages`]
//!   and the `du` inventory the same report collects, per declared root;
//! - **the remainder**, the inventory's own largest paths that no declared
//!   stage root covers, which is the list that was missing;
//! - **the mechanism**, because the reclamation stages are only one of this
//!   product's two. The janitor's cleaners are the other, and a path outside
//!   every stage root was printed as a path where nothing looks. On the same
//!   host that sentence was false: `~/.stado/local-storage` at 52.4 GiB holds
//!   the root of `release_store` and `~/.stado/local-backup` at 10.4 GiB is
//!   the root of `backup_twins`, and that host declares both. "A pass can
//!   reach this and stopped at its budget" and "nothing in this product can
//!   reach this" are opposite repairs.
//!
//! Nothing here reads a second source: the stage roots come from the one
//! compiled stage declaration the reclamation itself selects from, the cleaner
//! roots from [`crate::providers::local::disk_cleanup::catalogue`], and the
//! bytes from the one host read the report already performs.

mod paths;
mod render;
mod section;

pub use render::print_coverage;
pub use section::mechanisms::DeclaredCleaner;
pub use section::section;

/// How many paths outside the stage roots the report names. The list is an
/// operator's next action, not an inventory dump: the `du` read is already
/// capped per root, and a screen of rows buries the ones that matter.
const UNCOVERED_ROWS: usize = 12;
