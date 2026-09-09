//! Reclaim regenerable build caches on a registry-managed host.
//!
//! The disk cleaner knows two consumers, `huggingface_cache` and
//! `weles_recordings`, and neither covers what actually fills a developer
//! host: build output. A macbook ran out of space mid-link with 386 MiB free
//! while `disk-cleanup` reported a healthy no-op, because 6.8 GiB of `target/`
//! directories are invisible to it.
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
    AGE_ENV, APPLY_ENV, CACHEDIR_SIGNATURE, FORCE_ENV, REMOTE_SCRIPT, ROOT_ENV, STATUS_PREFIX,
};
pub use read::{declared_for_target, report_declaration_on_host, run_on_host};
pub use report::{parse_report, BuildCacheDeclaration, BuildCacheReport, CacheEntry};
