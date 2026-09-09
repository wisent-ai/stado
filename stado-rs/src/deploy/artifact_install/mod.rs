//! Materialise a published artifact onto a host, so a unit can point at a
//! version rather than at whatever happens to be on disk.
//!
//! `stado service deploy --from PATH` takes the absolute path, on the target
//! host, of the program the unit runs. That is deliberate — the command
//! manages units, not contents — but it leaves a gap nothing else in the pack
//! fills: no system owns getting a build onto a host. The consequence is
//! visible on any machine that has been running a while. Service directories
//! accumulate `current` as a plain copied directory beside hand-named backups
//! like `current.before-<change>-<timestamp>`, there is no version identity to
//! report, and nothing can say which build is running or what it is compatible
//! with. On 2026-08-04 a Skarbiec rebuilt in place began answering
//! `400 field required` to clients that had not moved with it, and took out a
//! health beacon and a gateway on the same host; no lineage existed to consult
//! because neither side was a published artifact.
//!
//! The two halves of the answer already exist. `stado artifact` publishes
//! immutable versioned manifests with aliases and lineage, and `service deploy`
//! renders a unit around a path. This module is the join: resolve an alias to
//! an immutable version, place exactly that version on the host under a path
//! that names it, verify the digest the manifest declares, and move `current`
//! onto it atomically. The unit then points at `current`, so a rollback is a
//! relink rather than a rebuild.
//!
//! The shapes a caller names live here. `validate` refuses a name, a version
//! or a manifest that must not reach a host, `script` holds the remote
//! program, and `install` is the sequence that runs it.

mod install;
mod script;
mod validate;

pub use install::{install_artifact, resolve_program_path};

/// Where a materialised service version lands, relative to the host's home.
pub const SERVICES_ROOT: &str = ".stado/services";

/// Everything the caller needs after a successful install: the path the unit
/// must run, and the immutable version now behind `current`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstalledArtifact {
    pub program_path: String,
    pub version: String,
    pub sha256: String,
}
