//! Target-scoped delivery of one local file or directory into a managed area
//! of the registry-approved account's home.
//!
//! The destination is home-relative by construction: callers know a Stado
//! target and a managed relative path, never its SSH account or home. Before
//! any byte is transferred, the host checks every existing destination
//! component with `lstat` semantics (`test -L`), refuses foreign ownership or
//! the wrong file kind, and reserves a same-parent staging path. `rsync -a`
//! carries uncommitted working-tree bytes, application bundles, modes and
//! symlinks through the target's selected Stado SSH route. Only after rsync
//! succeeds does one guarded rename replace the destination.

mod deliver;
mod plan;
mod script;
mod stages;

pub use deliver::deliver_host;
pub use script::DELIVERED_STATUS;
