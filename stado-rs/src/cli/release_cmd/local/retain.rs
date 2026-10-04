//! What a local install leaves behind after placing its binary: the
//! delivered archive retained beside the release, and the delivered version
//! declared for this host in the registry.

use std::path::Path;

use crate::cli::CmdError;

/// The bytes of the one regular `member` of a gzip'd release archive,
/// matched by its path or by a path ending in `/<member>`.
pub(super) fn extract_member(archive: Vec<u8>, member: &str) -> Result<Vec<u8>, CmdError> {
    let decoder = flate2::read::GzDecoder::new(std::io::Cursor::new(archive));
    let mut bundle = tar::Archive::new(decoder);
    let member = member.trim_start_matches('/');
    for entry in bundle.entries().map_err(|error| {
        CmdError::click(format!("unreadable release archive: {error}"))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    })? {
        let mut entry = entry.map_err(|error| {
            CmdError::click(format!("unreadable archive entry: {error}"))
                .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        let path = entry
            .path()
            .map_err(|error| {
                CmdError::click(format!("unreadable archive path: {error}"))
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?
            .to_string_lossy()
            .into_owned();
        if !entry.header().entry_type().is_file() {
            continue;
        }
        if path == member || path.ends_with(&format!("/{member}")) {
            let mut content = Vec::new();
            std::io::Read::read_to_end(&mut entry, &mut content).map_err(|error| {
                CmdError::click(format!("cannot extract {member}: {error}"))
                    .stating(crate::primitives::failure::FailureCode::InfraDown)
            })?;
            return Ok(content);
        }
    }
    Err(CmdError::click(format!(
        "release archive carries no regular member {member}"
    ))
    .stating(crate::primitives::failure::FailureCode::NotFound))
}

/// Where a delivered Stado archive of `version` is kept on this host, the
/// directory created: `~/.stado/releases/stado/<version>/<platform>/`.
pub(super) fn retained_archive_path(
    home: &Path,
    version: &str,
) -> Result<std::path::PathBuf, CmdError> {
    let platform = crate::self_update::platform_triple_short()
        .map_err(|error| CmdError::declaration(error.to_string()))?;
    let retained_dir = home
        .join(".stado")
        .join("releases")
        .join("stado")
        .join(version)
        .join(platform);
    std::fs::create_dir_all(&retained_dir).map_err(|error| {
        CmdError::click(format!(
            "cannot prepare retained Stado archive directory {}: {error}",
            retained_dir.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    Ok(retained_dir.join(crate::deploy::host_release::READER_ARCHIVE_NAME))
}

/// Keep the delivered archive beside the release it installed.
///
/// A rename cannot cross filesystems. Copying and then removing the source
/// retains the same archive when the job tree and release directory live
/// on different volumes.
pub(super) fn retain_archive(archive: &Path, destination: &Path) -> Result<(), CmdError> {
    if std::fs::rename(archive, destination).is_ok() {
        return Ok(());
    }
    std::fs::copy(archive, destination).map_err(|error| {
        CmdError::click(format!(
            "cannot retain delivered Stado archive at {}: {error}",
            destination.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    std::fs::remove_file(archive).map_err(|error| {
        CmdError::click(format!(
            "retained the delivered Stado archive at {} but could not remove {}: {error}",
            destination.display(),
            archive.display()
        ))
        .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })
}

/// Write the version this delivery installed into the host's own
/// `targets[].managed_versions`, so the declaration follows the delivery.
///
/// A delivery that leaves the declaration where it was makes every later
/// `release host-state` read `host-ahead: the declaration is stale, not the
/// host` and refuse to deliver anything until an operator moves the
/// declaration by hand. The delivery is the fact; the declaration records it.
pub(super) async fn declare_delivered_version(binary: &str, version: &str) -> Result<(), CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let binary = binary.to_string();
    let version = version.to_string();
    let generation = crate::cli::registry::commit_document(move |current| {
        let registry = crate::targets::load_registry_from_str(&serde_json::to_string(current)?)
            .map_err(CmdError::from)?;
        let target = registry
            .lookup_self(&hostname)
            .map_err(CmdError::from)?
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{hostname} has no registry target identity; the delivered {binary} \
                     {version} cannot be declared for it"
                ))
                .stating(crate::primitives::failure::FailureCode::NotFound)
            })?;
        let target_name = target.name.clone();
        let mut next = current.clone();
        let entry = next
            .get_mut("targets")
            .and_then(serde_json::Value::as_array_mut)
            .and_then(|targets| {
                targets.iter_mut().find(|candidate| {
                    candidate.get("name").and_then(serde_json::Value::as_str)
                        == Some(target_name.as_str())
                })
            })
            .and_then(serde_json::Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(format!("{target_name} is missing from registry.targets"))
                    .stating(crate::primitives::failure::FailureCode::NotFound)
            })?;
        let versions = entry
            .entry("managed_versions".to_string())
            .or_insert_with(|| serde_json::Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{target_name} declares managed_versions as a non-object"
                ))
                .stating(crate::primitives::failure::FailureCode::Config)
            })?;
        versions.insert(binary.clone(), serde_json::Value::String(version.clone()));
        Ok(next)
    })
    .await
    .map_err(|error| {
        let mut wrapped = CmdError::click(format!(
            "the release is installed, but declaring it under targets[].managed_versions \
             failed: {error}"
        ));
        wrapped.failure = error.failure;
        wrapped
    })?;
    println!("declared under managed_versions (registry generation {generation})");
    Ok(())
}
