//! The registry document a leased target is declared in.
//!
//! This is the half that makes a scratch target usable by a test. A test
//! isolates its DECLARATION — its own store root, its own registry, a
//! `STADO_CONFIG` pointing at nothing — while the effect stays real, so what
//! it needs is a registry document naming one machine it is allowed to touch.
//! `create` writes exactly that, and nothing else, into a directory the caller
//! names.
//!
//! The document is validated with the registry-v2 contract before it is
//! written. A document Stado itself would refuse must never reach a test,
//! because the failure it produces there looks like the capability under test
//! failing.

use std::path::{Path, PathBuf};

use serde_json::{json, Value};

use super::lease::ScratchLease;
use crate::deploy::DeployError;
use crate::targets::{ComputeTarget, REGISTRY_SCHEMA_VERSION};

/// The one object every backend reads a registry from, store-relative.
pub const REGISTRY_FILE: &str = "registry.json";

/// The role a leased target declares, so a document found later says what it
/// was for without anyone having to guess from the name.
pub const SCRATCH_ROLE: &str = "scratch";

/// Build the document declaring one leased target.
///
/// `hostnames` is deliberately empty. The channel treats a target whose name
/// or hostnames match this machine as local and runs the program directly, as
/// the login account — which on the leasing host would silently run every
/// command as the operator instead of the leased account, and quietly undo the
/// isolation the lease exists to provide. With no hostnames the ssh hop always
/// happens, and the account that runs the command is the one in the `ssh`
/// destination.
pub fn document(lease: &ScratchLease, parent: &ComputeTarget, ssh: &str) -> Value {
    json!({
        "schema_version": REGISTRY_SCHEMA_VERSION,
        "coordinators": [],
        "targets": [{
            "name": lease.name,
            "kind": "local",
            "ssh": ssh,
            // One machine, one channel key: the leased account trusts the keys
            // that already reach this host, so it authenticates with the key
            // minted under the host's name rather than an identity of its own.
            "ssh_key_target": parent.name,
            "release_platform": parent.release_platform,
            "role": SCRATCH_ROLE,
            "notes": format!(
                "leased by `stado scratch create` on {}; expires {}; destroyed by `stado scratch destroy {}`",
                parent.name, lease.expires_at, lease.name
            ),
        }],
    })
}

/// Write the document into a fresh store root and return its path.
///
/// An existing root is refused rather than overwritten: two runs sharing one
/// root would share one registry, and the second would silently retarget the
/// first.
pub fn write(
    root: &Path,
    lease: &ScratchLease,
    parent: &ComputeTarget,
    ssh: &str,
) -> Result<PathBuf, DeployError> {
    if root.exists() {
        return Err(DeployError(format!(
            "{} already exists; refusing to write a scratch registry over it",
            root.display()
        )));
    }
    let document = document(lease, parent, ssh);
    crate::targets::validate_registry(&document).map_err(|exc| {
        DeployError(format!(
            "the scratch registry this build renders is not a valid registry document: {exc}"
        ))
    })?;
    std::fs::create_dir_all(root)
        .map_err(|exc| DeployError(format!("{} is not creatable: {exc}", root.display())))?;
    let path = root.join(REGISTRY_FILE);
    let body = serde_json::to_string_pretty(&document)
        .map_err(|exc| DeployError(format!("scratch registry is not serializable: {exc}")))?;
    std::fs::write(&path, format!("{body}\n"))
        .map_err(|exc| DeployError(format!("{} is not writable: {exc}", path.display())))?;
    Ok(path)
}

/// Remove one lease's store root, and say whether there was anything to
/// remove. Only ever called for a root this capability created.
pub fn remove(root: &Path) -> Result<&'static str, DeployError> {
    if !root.exists() {
        return Ok("absent");
    }
    std::fs::remove_dir_all(root)
        .map_err(|exc| DeployError(format!("{} is not removable: {exc}", root.display())))?;
    Ok("removed")
}
