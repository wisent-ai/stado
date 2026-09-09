//! Auto-redirect agent staging away from a RAM-backed /tmp.
//!
//! Port of `stado/providers/local/disk/staging.py`.
//!
//! When `/tmp` is a tmpfs (every byte staged there counts as RAM), the
//! agent's multi-GB raw-activation jobs accumulate in RAM, drive the
//! process to OOM territory, and exit `status=1` — losing every in-flight
//! job to requeue. This module runs at agent startup, detects that
//! condition, and points TMPDIR at the largest disk-backed mount the agent
//! user can actually traverse and write, creating a `wisent-staging`
//! subdir there. Children inherit the env, so every job stages on disk for
//! free. (Python also assigns `tempfile.tempdir`; Rust has no analog — the
//! env var is what children inherit.)
//!
//! Fully automatic, resource-linked. No hardcoded paths, no concurrency
//! cap. No-op when /tmp is already disk-backed or TMPDIR is already set to
//! a non-/tmp path. When running as root the agent can also chmod o+x
//! parent dirs to recover an otherwise-skipped large mount (e.g. a Vast
//! host where /var/lib/docker is 0710).
//!
//! One module, kept in parts small enough to read, grouped by the seams the
//! redirect already had: `mounts` (which mounts qualify, what room they
//! have, whether this user can write there, and the root-only traversal
//! repair), `identity` (the agent user and the ownership handover),
//! `rotational` (SSD-or-HDD for a candidate's backing device) and
//! `setup` (the startup entry point that ranks the candidates and
//! publishes the winner). Every part opens with `use super::*;`, so the
//! imports below are the module's single import list and a part sees the
//! items of every other part exactly as it did when this was one file.
//! Each part is re-exported by glob, so
//! `crate::providers::local::disk_staging::<item>` still names every item
//! it named before.

use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

mod identity;
mod mounts;
mod rotational;
mod setup;

pub use identity::*;
pub use mounts::*;
pub use rotational::*;
pub use setup::*;
