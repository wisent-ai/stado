//! The disk measured against the disk-full rule: how far the volume is from
//! the threshold, what the fleet's cleaners reach, and what nothing reaches.
//!
//! A reading can be true and useless: a host with a few hundred MB free, a
//! report printing `99%`, and a delivery dying with `No space left on
//! device`. So the report measures:
//!
//! - **the verdict**, from the rule ([`crate::providers::local::disk_cleanup::rule`])
//!   applied to the host's own `df` reading;
//! - **the coverage**, from [`crate::deploy::host_reclaim::declared_stages`]
//!   and the `du` inventory the same report collects, per stage root;
//! - **the mechanism**, the janitor cleaner whose area holds a path, from the
//!   rule's own cleaner list;
//! - **the remainder**, the inventory's largest paths no stage and no cleaner
//!   reaches: the user's data, which the rule never takes.

mod paths;
mod render;
mod section;

pub use render::print_coverage;
pub use section::section;

/// How many paths outside the stage roots the report names. The list is an
/// operator's next action, not an inventory dump: the `du` read is already
/// capped per root, and a screen of rows buries the ones that matter.
const UNCOVERED_ROWS: usize = 12;
