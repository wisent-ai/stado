//! Reclaim regenerable build caches on a registry-managed host.
//!
//! Two halves, because they answer to different things. [`script`] is the
//! remote program, its environment contract and its parsers: no host, no
//! registry, nothing to reach. [`hosts`] is what a machine sees — the cleanup
//! declaration one target resolves to, and the two commands that read or prune
//! with it.
//!
//! The safety rule lives with the program in [`script`]: a directory is
//! deletable only when it carries a `CACHEDIR.TAG` whose first line holds the
//! Cache Directory Tagging Standard signature, which cargo and many other
//! build tools write precisely so a cleaner may remove the directory without
//! asking. No name matching and no extension lists.

mod hosts;
mod script;

pub use hosts::{declared_for_target, report_declaration_on_host, run_on_host};
pub use script::{
    parse_report, remote_command, validate_days, validate_root, BuildCacheDeclaration,
    BuildCacheReport, CacheEntry, AGE_ENV, APPLY_ENV, CACHEDIR_SIGNATURE, FORCE_ENV, REMOTE_SCRIPT,
    ROOT_ENV, STATUS_PREFIX,
};
