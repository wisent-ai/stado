//! `service handoff`: moving one placed service from generic
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

/// Write the handoff receipt whole and durably through the one writer Stado
/// keeps its files with: over what is there when `replace`, else only where
/// no receipt is. A receipt already there with the same bytes is the same
/// handoff and stands; one with other bytes is refused. A private copy read
/// every failed no-clobber placement as "already there", so a refused link
/// was answered by reading a file that did not exist.
fn persist_handoff_receipt(
    path: &std::path::Path,
    report: &Value,
    replace: bool,
) -> Result<(), CmdError> {
    let mut bytes = serde_json::to_vec_pretty(report)?;
    bytes.push(b'\n');
    let mode = crate::primitives::file_mode::owner_read_write();
    if replace {
        crate::primitives::document::persist_with_mode(path, &bytes, mode)?;
        return Ok(());
    }
    if crate::primitives::document::persist_if_absent(path, &bytes, mode)? {
        return Ok(());
    }
    if std::fs::read(path)? != bytes {
        return Err(CmdError::refused(format!(
            "handoff receipt {} already exists with different content",
            path.display()
        )));
    }
    Ok(())
}

fn read_handoff_receipt(path: &std::path::Path) -> Result<Option<Value>, CmdError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes).map_err(|error| {
            CmdError::unreachable(format!(
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
    legacy_identities: &[&str],
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
        .iter()
        .all(|identity| !identity.is_empty() && !document_contains_string(document, identity));
    units_external && legacy_removed && legacy_unreachable
}

fn same_remote_file_identity(left: &Value, right: &Value) -> bool {
    ["path", "sha256", "size", "mode"]
        .into_iter()
        .all(|field| left.get(field) == right.get(field))
}
