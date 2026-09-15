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
const LEASE_FILE: &str = "scratch-lease.json";

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

/// The fleet's release trust, trimmed for one lease's document, and the
/// sentence `create` reports either way.
///
/// A signed release is verified against `release_control.trusted_keys` in the
/// registry the DELIVERING side reads, and for a leased run that is this
/// one-target document. Carrying the fleet's public keys is what lets a
/// disposable target be delivered a real pipeline-signed version; without them
/// `host-state --apply` refuses every such version with `registry declares no
/// release trust keys`, which is how this capability shipped and why only
/// legacy-manifest versions could reach a lease.
///
/// `products` is emptied on the way through. Desired state is the fleet's, not
/// the lease's: nothing reconciles a throwaway account, and a copied policy
/// would name logical services this document does not declare. Trust travels;
/// desired state does not.
pub fn trust(document: &Value) -> (Option<Value>, String) {
    // `none: ` prefixes every answer that is not a key list, so one field can
    // carry both without a reader having to guess which it got. The CLI prints
    // it verbatim and the Desktop keys its tone off the prefix.
    let mut control = match crate::release_control::control(document) {
        Ok(Some(control)) => control,
        Ok(None) => {
            return (
                None,
                "none: the fleet declares no release trust".to_string(),
            )
        }
        Err(error) => {
            return (
                None,
                format!("none: the fleet's release trust does not parse: {error}"),
            )
        }
    };
    if control.trusted_keys.is_empty() {
        return (
            None,
            "none: the fleet declares no release trust keys".to_string(),
        );
    }
    control.products.clear();
    let ids = control
        .trusted_keys
        .keys()
        .cloned()
        .collect::<Vec<_>>()
        .join(", ");
    match serde_json::to_value(&control) {
        Ok(value) => (Some(value), ids),
        Err(error) => (
            None,
            format!("none: the fleet's release trust is not serializable: {error}"),
        ),
    }
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
    trust: Option<Value>,
) -> Result<PathBuf, DeployError> {
    if root.symlink_metadata().is_ok() {
        return Err(DeployError(format!(
            "{} already exists; refusing to write a scratch registry over it",
            root.display()
        )));
    }
    let mut document = document(lease, parent, ssh);
    if let Some(trust) = trust {
        document[crate::release_control::RELEASE_CONTROL_KEY] = trust;
    }
    crate::targets::validate_registry(&document).map_err(|exc| {
        DeployError(format!(
            "the scratch registry this build renders is not a valid registry document: {exc}"
        ))
    })?;
    // The release contract as well as the registry contract, because the trust
    // block above is the half a delivery reads: a document that satisfies one
    // and not the other fails inside `host-state --apply`, three commands away
    // from the call that wrote it.
    crate::release_control::validate_registry_contract(&document).map_err(|exc| {
        DeployError(format!(
            "the scratch registry this build renders does not satisfy the release contract: {exc}"
        ))
    })?;
    std::fs::create_dir_all(root)
        .map_err(|exc| DeployError(format!("{} is not creatable: {exc}", root.display())))?;
    let path = root.join(REGISTRY_FILE);
    let body = serde_json::to_string_pretty(&document)
        .map_err(|exc| DeployError(format!("scratch registry is not serializable: {exc}")))?;
    std::fs::write(&path, format!("{body}\n"))
        .map_err(|exc| DeployError(format!("{} is not writable: {exc}", path.display())))?;
    let receipt = serde_json::to_vec(lease)
        .map_err(|error| DeployError(format!("scratch lease is not serializable: {error}")))?;
    std::fs::write(root.join(LEASE_FILE), receipt).map_err(|error| {
        DeployError(format!(
            "{} lease identity is not writable: {error}",
            root.display()
        ))
    })?;
    Ok(path)
}

/// Remove only the caller-local root identified by this lease. A different
/// machine can reap the remote account, but cannot judge the caller's disk.
pub fn remove(lease: Option<&ScratchLease>) -> Result<&'static str, DeployError> {
    let Some(lease) = lease else {
        return Ok("unrecorded");
    };
    if lease.requested_by != super::lease::requested_by() {
        return Ok("on-another-caller");
    }
    let default_root;
    let root = match lease.storage_root.as_deref() {
        Some(root) => root,
        None => {
            default_root = super::lease::local_root(&lease.name);
            &default_root
        }
    };
    let metadata = match root.symlink_metadata() {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok("absent"),
        Err(error) => {
            return Err(DeployError(format!(
                "{} is not readable: {error}",
                root.display()
            )))
        }
    };
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(DeployError(format!(
            "{} is not a real scratch directory; nothing there was removed",
            root.display()
        )));
    }
    if lease.storage_root.is_some() {
        let bytes = std::fs::read(root.join(LEASE_FILE)).map_err(|error| {
            DeployError(format!(
                "{} lease identity is not readable: {error}",
                root.display()
            ))
        })?;
        let identity: ScratchLease = serde_json::from_slice(&bytes).map_err(|error| {
            DeployError(format!(
                "{} lease identity is invalid: {error}",
                root.display()
            ))
        })?;
        if identity != *lease {
            return Err(DeployError(format!(
                "{} belongs to another scratch lease; nothing there was removed",
                root.display()
            )));
        }
    } else {
        // Existing v1 records predate the explicit root. Their default root
        // must still contain the matching scratch target before it can go.
        let bytes = std::fs::read(root.join(REGISTRY_FILE)).map_err(|error| {
            DeployError(format!(
                "{} registry is not readable: {error}",
                root.display()
            ))
        })?;
        let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
            DeployError(format!("{} registry is invalid: {error}", root.display()))
        })?;
        let targets = document["targets"].as_array();
        if !targets.is_some_and(|targets| {
            targets.len() == 1
                && targets[0]["name"] == lease.name
                && targets[0]["role"] == SCRATCH_ROLE
                && targets[0]["ssh_key_target"] == lease.target
        }) {
            return Err(DeployError(format!(
                "{} does not identify this scratch lease; nothing there was removed",
                root.display()
            )));
        }
    }
    std::fs::remove_dir_all(root)
        .map_err(|error| DeployError(format!("{} is not removable: {error}", root.display())))?;
    Ok("removed")
}
