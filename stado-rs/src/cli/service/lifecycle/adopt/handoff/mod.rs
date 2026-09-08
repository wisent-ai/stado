//! `service handoff-release-control`: moving one placed service from generic
//! unit lifecycle to its active signed release.
//!
//! The durable receipt beside the registry document is what makes the
//! transfer resumable: this module owns writing it, reading it back, and
//! deciding whether the registry already carries the handoff it records.

use super::*;

mod commit;
pub(crate) mod control;
mod document;
mod identity;

use commit::{finish_committed_handoff, finish_handoff_under_lease};
use document::{externalize_release_controlled_profile, remove_release_legacy_identity};
use identity::{remote_file_identity, require_no_executable_caller};

fn document_contains_string(value: &Value, needle: &str) -> bool {
    match value {
        Value::String(value) => value == needle,
        Value::Array(values) => values
            .iter()
            .any(|value| document_contains_string(value, needle)),
        Value::Object(values) => values
            .values()
            .any(|value| document_contains_string(value, needle)),
        _ => false,
    }
}
fn handoff_receipt_path(product: &str, version: &str, host: &str) -> std::path::PathBuf {
    crate::config_file::expand_tilde("~")
        .join(".stado/work/service-release")
        .join(product)
        .join(version)
        .join(format!("handoff-{host}.json"))
}

fn persist_handoff_receipt(
    path: &std::path::Path,
    report: &Value,
    replace: bool,
) -> Result<(), CmdError> {
    use std::io::Write as _;

    let parent = path
        .parent()
        .ok_or_else(|| CmdError::click("handoff receipt path has no parent"))?;
    std::fs::create_dir_all(parent)?;
    let mut bytes = serde_json::to_vec_pretty(report)?;
    bytes.push(b'\n');
    let mut staged = tempfile::NamedTempFile::new_in(parent)?;
    staged.write_all(&bytes)?;
    staged.as_file().sync_all()?;
    if replace {
        staged.persist(path).map_err(|error| error.error)?;
    } else if let Err(error) = staged.persist_noclobber(path) {
        let existing = std::fs::read(path)?;
        if existing != bytes {
            return Err(CmdError::click(format!(
                "handoff receipt {} already exists with different content",
                path.display()
            )));
        }
        drop(error);
    }
    std::fs::File::open(parent)?.sync_all()?;
    Ok(())
}

fn read_handoff_receipt(path: &std::path::Path) -> Result<Option<Value>, CmdError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).map_err(|error| {
            CmdError::click(format!(
                "handoff receipt {} is invalid JSON: {error}",
                path.display()
            ))
        })?)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn registry_has_intended_handoff(
    document: &Value,
    profile: &str,
    service_name: &str,
    product: &str,
    host: &str,
    legacy_identities: [&str; 3],
) -> bool {
    let units_external = document
        .get("placement_profiles")
        .and_then(Value::as_array)
        .and_then(|profiles| {
            profiles
                .iter()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(profile))
        })
        .and_then(|profile| profile.get("hosts"))
        .and_then(Value::as_object)
        .map(|hosts| {
            !hosts.is_empty()
                && hosts.values().all(|template| {
                    template
                        .get("units")
                        .and_then(|units| units.get(service_name))
                        .is_some_and(|unit| {
                            unit.get("controller").and_then(Value::as_str)
                                == Some("release-control")
                                && unit.get("product").and_then(Value::as_str) == Some(product)
                        })
                })
        })
        == Some(true);
    let legacy_removed = document
        .get("release_control")
        .and_then(|control| control.get("products"))
        .and_then(|products| products.get(product))
        .and_then(|policy| policy.get("targets"))
        .and_then(|targets| targets.get(host))
        .is_some_and(|target| {
            target.get("legacy_launchd_label").is_none()
                && target.get("legacy_launchd_plist").is_none()
        });
    let legacy_unreachable = legacy_identities
        .into_iter()
        .all(|identity| !identity.is_empty() && !document_contains_string(document, identity));
    units_external && legacy_removed && legacy_unreachable
}

fn same_remote_file_identity(left: &Value, right: &Value) -> bool {
    ["path", "sha256", "size", "mode"]
        .into_iter()
        .all(|field| left.get(field) == right.get(field))
}
