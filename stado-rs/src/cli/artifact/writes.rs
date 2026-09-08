//! The artifact verbs that change the registry: `import activations`,
//! `publish` and `alias set`.
//!
//! `publish` additionally refuses a fleet coordinate whose backing store only
//! answers for this machine; see `fleet_visible`.

use std::path::Path;

use serde_json::{Map, Value};

use crate::artifacts::adapters::build_activation_manifest;
use crate::artifacts_models::ArtifactManifest;

use crate::cli::CmdError;

use super::format::{json_pretty_sorted, json_sorted, parse_ref};
use super::registry;

#[allow(clippy::too_many_arguments)]
pub(super) async fn import_activations(
    repo: &str,
    revision: &str,
    desired_state_dir: &str,
    run_id: &str,
    job_ids: &[String],
    version: &str,
    aliases: &[String],
    full: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    // click.Path(exists=True, file_okay=False): usage error, exit 2.
    let dir = Path::new(desired_state_dir);
    if !dir.is_dir() {
        return Err(CmdError {
            message: Some(format!(
                "Invalid value for '--desired-state-dir': Directory '{desired_state_dir}' does not exist."
            )),
            code:
                2,
            ..CmdError::default()
        });
    }
    let manifest = build_activation_manifest(repo, revision, dir, run_id, job_ids, version)
        .map_err(CmdError::click)?;
    let registry = registry().await?;
    let published = registry.publish(&manifest, true, full).await?;
    let mut alias_refs = Vec::with_capacity(aliases.len());
    for alias in aliases {
        let alias_ref = registry.set_alias(&published.ref_, alias, None, "").await?;
        alias_refs.push(alias_ref.to_string());
    }
    if as_json {
        let mut value = published.to_dict();
        value
            .as_object_mut()
            .expect("to_dict is an object")
            .insert("aliases_created".into(), Value::from(alias_refs));
        println!("{}", json_pretty_sorted(&value));
    } else {
        println!("{}", published.ref_);
        for alias_ref in &alias_refs {
            println!("{alias_ref} -> {}", published.ref_);
        }
    }
    Ok(())
}

/// A release is a claim about the fleet, so it cannot rest on a store only one
/// machine can read.
///
/// `stado://` resolves through whichever object store this host is configured
/// with, and the default is a directory on this disk. Publishing a fleet
/// coordinate backed by that store does not fail -- it succeeds, and produces a
/// version every other machine reports as absent. The store is the operator's
/// choice and stays that way; what is refused here is only the combination of a
/// fleet coordinate with a store that cannot answer for the fleet.
fn fleet_visible(manifest: &ArtifactManifest) -> Result<(), CmdError> {
    let fleet_scheme = manifest
        .locations
        .iter()
        .any(|location| location.uri.starts_with("stado://"));
    if !fleet_scheme {
        return Ok(());
    }
    let backend = crate::config::wc_storage_backend();
    // Ask the store how far it carries. Every storage adapter declares this, so
    // a backend added later is classified by whoever adds it rather than by
    // whether its name happens to be matched here.
    let reach = crate::capabilities::storage_reach(backend);
    match reach {
        Some(crate::capabilities::StorageReach::Fleet) => Ok(()),
        Some(crate::capabilities::StorageReach::Device) => Err(CmdError::click(format!(
            "{} publishes a stado:// coordinate while this host's object store is {backend:?}, \
             which answers only for this machine: every other host would report the release \
             absent. Select a store that answers for the fleet, or give the manifest a \
             location the fleet can already reach.",
            manifest.ref_
        ))),
        None => Err(CmdError::click(format!(
            "{} publishes a stado:// coordinate, and this host's object store {backend:?} is \
             not a storage backend this build knows, so how far it carries cannot be \
             established",
            manifest.ref_
        ))),
    }
}

pub(super) async fn publish(
    manifest_path: &str,
    verify: bool,
    full: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    // click.Path(exists=True, dir_okay=False): usage error, exit 2.
    let path = Path::new(manifest_path);
    if !path.is_file() {
        return Err(CmdError {
            message: Some(format!(
                "Invalid value for 'MANIFEST_PATH': File '{manifest_path}' does not exist."
            )),
            code: 2,
            ..CmdError::default()
        });
    }
    let manifest = ArtifactManifest::from_json(&std::fs::read_to_string(path)?)?;
    fleet_visible(&manifest)?;
    let registry = registry().await?;
    let published = registry.publish(&manifest, verify, full).await?;
    if as_json {
        println!("{}", json_pretty_sorted(&published.to_dict()));
    } else {
        println!("{}", published.ref_);
    }
    Ok(())
}

pub(super) async fn alias_set(
    target_ref: &str,
    alias: &str,
    expected_previous: Option<&str>,
    as_json: bool,
) -> Result<(), CmdError> {
    let registry = registry().await?;
    let alias_ref = registry
        .set_alias(&parse_ref(target_ref)?, alias, expected_previous, "")
        .await?;
    let resolved = registry.resolve(&alias_ref).await?;
    if as_json {
        let value = Value::Object(Map::from_iter([
            ("alias_ref".into(), Value::from(alias_ref.to_string())),
            ("resolved_ref".into(), Value::from(resolved.to_string())),
        ]));
        println!("{}", json_sorted(&value));
    } else {
        println!("{alias_ref} -> {resolved}");
    }
    Ok(())
}
