//! Unpack a verified release archive into its immutable directory, and name
//! the directory one release installs into on a target.

use std::fs::File;
use std::io::Read;
use std::path::{Component, Path, PathBuf};

use crate::release_control::{
    ProductReleasePolicy, ReleaseManifest, ReleaseTargetPolicy, MAX_ARCHIVE_ENTRIES,
    MAX_EXTRACTED_BYTES, MAX_RELEASE_BYTES, MAX_SOURCE_ARCHIVE_ENTRIES,
};

pub fn safe_extract_archive(bytes: &[u8], destination: &Path) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_RELEASE_BYTES {
        return Err("release archive size is outside the supported range".to_string());
    }
    safe_extract_archive_reader(bytes, destination, MAX_ARCHIVE_ENTRIES)
}

/// Extract a source snapshot - a whole repository, not a release payload -
/// under the same byte and path rules and the entry bound sized for one.
pub fn safe_extract_source_archive(bytes: &[u8], destination: &Path) -> Result<(), String> {
    if bytes.is_empty() || bytes.len() as u64 > MAX_RELEASE_BYTES {
        return Err("source archive size is outside the supported range".to_string());
    }
    safe_extract_archive_reader(bytes, destination, MAX_SOURCE_ARCHIVE_ENTRIES)
}

/// Extract an already-verified archive without reading it back into memory.
/// The exact signed byte count is checked again at the file boundary.
pub fn safe_extract_archive_file(
    archive_path: &Path,
    expected_bytes: u64,
    destination: &Path,
) -> Result<(), String> {
    let file = File::open(archive_path).map_err(|error| {
        format!(
            "cannot open release archive {}: {error}",
            archive_path.display()
        )
    })?;
    let metadata = file.metadata().map_err(|error| {
        format!(
            "cannot stat release archive {}: {error}",
            archive_path.display()
        )
    })?;
    if !metadata.is_file()
        || expected_bytes == 0
        || expected_bytes > MAX_RELEASE_BYTES
        || metadata.len() != expected_bytes
    {
        return Err("release archive size differs from its signed manifest".to_string());
    }
    safe_extract_archive_reader(file, destination, MAX_ARCHIVE_ENTRIES)
}

fn safe_extract_archive_reader(
    reader: impl Read,
    destination: &Path,
    max_entries: usize,
) -> Result<(), String> {
    if destination.exists() {
        return Err(format!(
            "immutable release directory already exists: {}",
            destination.display()
        ));
    }
    let parent = destination
        .parent()
        .ok_or_else(|| "release destination has no parent".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("cannot create release parent {}: {error}", parent.display()))?;
    let staging = parent.join(format!(".release-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir(&staging).map_err(|error| {
        format!(
            "cannot create release staging {}: {error}",
            staging.display()
        )
    })?;
    let result = (|| {
        let decoder = flate2::read::GzDecoder::new(reader);
        let mut archive = tar::Archive::new(decoder);
        let entries = archive
            .entries()
            .map_err(|error| format!("cannot read release archive: {error}"))?;
        let mut count = 0_usize;
        let mut extracted_bytes = 0_u64;
        for entry in entries {
            count += 1;
            if count > max_entries {
                return Err(format!("release archive exceeds {max_entries} entries"));
            }
            let mut entry = entry.map_err(|error| format!("cannot read release entry: {error}"))?;
            let archived_path = entry
                .path()
                .map_err(|error| format!("invalid release entry path: {error}"))?
                .into_owned();
            let mut path = PathBuf::new();
            for component in archived_path.components() {
                match component {
                    Component::Normal(segment) => path.push(segment),
                    Component::CurDir => {}
                    Component::ParentDir | Component::RootDir | Component::Prefix(_) => {
                        return Err(format!(
                            "unsafe release entry path: {}",
                            archived_path.display()
                        ));
                    }
                }
            }
            let kind = entry.header().entry_type();
            if kind.is_pax_global_extensions() || kind.is_pax_local_extensions() {
                continue;
            }
            if path.as_os_str().is_empty() {
                if kind.is_dir() {
                    continue;
                }
                return Err("release archive contains an empty file path".to_string());
            }
            if !kind.is_file() && !kind.is_dir() {
                return Err(format!(
                    "release entry is not a regular file or directory: {}",
                    path.display()
                ));
            }
            if kind.is_file() {
                extracted_bytes = extracted_bytes
                    .checked_add(entry.header().size().map_err(|error| {
                        format!("invalid release entry size for {}: {error}", path.display())
                    })?)
                    .ok_or_else(|| "release archive expanded size overflowed".to_string())?;
                if extracted_bytes > MAX_EXTRACTED_BYTES {
                    return Err(format!(
                        "release archive expands beyond {MAX_EXTRACTED_BYTES} bytes"
                    ));
                }
            }
            let output = staging.join(&path);
            if kind.is_dir() {
                std::fs::create_dir_all(&output).map_err(|error| {
                    format!(
                        "cannot create release directory {}: {error}",
                        output.display()
                    )
                })?;
                continue;
            }
            if let Some(parent) = output.parent() {
                std::fs::create_dir_all(parent).map_err(|error| {
                    format!(
                        "cannot create release directory {}: {error}",
                        parent.display()
                    )
                })?;
            }
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&output)
                .map_err(|error| {
                    format!("cannot create release file {}: {error}", output.display())
                })?;
            std::io::copy(&mut entry, &mut file).map_err(|error| {
                format!("cannot extract release file {}: {error}", output.display())
            })?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                let source_mode = entry.header().mode().unwrap_or(0);
                let executable = source_mode & 0o111 != 0;
                let owner_only = source_mode & 0o077 == 0;
                let mode = match (executable, owner_only) {
                    (true, true) => 0o700,
                    (true, false) => 0o755,
                    (false, _) => 0o600,
                };
                std::fs::set_permissions(&output, std::fs::Permissions::from_mode(mode)).map_err(
                    |error| format!("cannot set release mode {}: {error}", output.display()),
                )?;
            }
        }
        if count == 0 {
            return Err("release archive is empty".to_string());
        }
        std::fs::rename(&staging, destination).map_err(|error| {
            format!(
                "cannot commit immutable release {} -> {}: {error}",
                staging.display(),
                destination.display()
            )
        })?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_dir_all(&staging);
    }
    result
}

pub fn install_root_path(policy: &ProductReleasePolicy, target: &ReleaseTargetPolicy) -> PathBuf {
    let expanded = policy.install_root.replace("{home}", &target.home);
    let path = PathBuf::from(expanded);
    if path.is_absolute() {
        path
    } else {
        Path::new(&target.home).join(path)
    }
}

pub fn release_directory(
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
    version: &str,
    platform: &str,
) -> PathBuf {
    install_root_path(policy, target)
        .join("releases")
        .join(version)
        .join(platform)
}

pub fn install_directory(
    policy: &ProductReleasePolicy,
    target: &ReleaseTargetPolicy,
    manifest: &ReleaseManifest,
) -> PathBuf {
    release_directory(policy, target, &manifest.version, &manifest.platform)
}
