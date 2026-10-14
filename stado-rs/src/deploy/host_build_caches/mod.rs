//! Reclaim regenerable build caches on a registry-managed host.
//!
//! The local reclaim rule's declared cleaners do not cover every build
//! output tree. This stage inventories cache-tagged build directories that
//! the fleet can regenerate and reports their actual size before removal.
//!
//! Safety comes from the Cache Directory Tagging Standard rather than from
//! guessing at directory names: a directory is deletable only when it contains
//! a `CACHEDIR.TAG` whose first line carries the standard signature. Cargo,
//! and many other build tools, write it precisely so that a cleaner may remove
//! the directory without asking. Nothing else is touched — no name matching,
//! no extension lists.
//!
//! `report` lists candidates with sizes; `prune` deletes them. Age and root
//! arrive as explicit arguments, so the command carries no threshold of its
//! own. A process snapshot plus `lsof +D` over the cache's owning project
//! protects every candidate immediately before deletion, including when an
//! operator explicitly overrides age.

mod invocation;
mod program;
mod read;
mod report;

pub use invocation::{remote_command, validate_days, validate_root};
pub use program::{
    AGE_ENV, APPLY_ENV, CACHEDIR_SIGNATURE, FORCE_ENV, REMOTE_SCRIPT, ROOT_ENV, SCAN_FAILED,
    STATUS_PREFIX,
};
pub use read::{home_scan_for_target, report_declaration_on_host, run_on_host};
pub use report::{parse_report, BuildCacheDeclaration, BuildCacheReport, CacheEntry};
