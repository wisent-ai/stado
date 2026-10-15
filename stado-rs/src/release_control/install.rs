//! Unpack a verified release archive into its immutable directory, and name
//! the directory one release installs into on a target.

use std::fs::File;
use std::io::Read;
use std::num::NonZeroU64;
use std::path::{Component, Path, PathBuf};

use nix::sys::stat::Mode;

use crate::release_control::{ProductReleasePolicy, ReleaseManifest, ReleaseTargetPolicy};

pub fn safe_extract_archive(bytes: &[u8], destination: &Path) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("release archive is empty".to_string());
    }
    safe_extract_archive_reader(bytes, destination)
}

/// Extract a source snapshot - a whole repository, not a release payload -
/// under the same path rules and the same measured room.
pub fn safe_extract_source_archive(bytes: &[u8], destination: &Path) -> Result<(), String> {
    if bytes.is_empty() {
        return Err("source archive is empty".to_string());
    }
    safe_extract_archive_reader(bytes, destination)
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
        || NonZeroU64::new(expected_bytes).is_none()
        || metadata.len() != expected_bytes
    {
        return Err("release archive size differs from its signed manifest".to_string());
    }
    safe_extract_archive_reader(file, destination)
}

/// What the destination volume can still hold, as its own `statvfs` answers
/// for an unprivileged writer: bytes and file entries. An archive expanding
/// past either is refused while it is read, before it fills the volume.
struct Room {
    bytes: u64,
    entries: u64,
}

fn room(path: &Path) -> Result<Room, String> {
    let stat = nix::sys::statvfs::statvfs(path).map_err(|error| {
        format!(
            "cannot measure the room left on {}: {error}",
            path.display()
        )
    })?;
    Ok(Room {
        bytes: (stat.blocks_available() as u64).saturating_mul(stat.fragment_size() as u64),
        entries: stat.files_available() as u64,
    })
}

fn safe_extract_archive_reader(reader: impl Read, destination: &Path) -> Result<(), String> {
    if crate::primitives::presence::present(destination).map_err(|error| error.to_string())? {
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
        let room = room(&staging)?;
        let decoder = flate2::read::GzDecoder::new(reader);
        let mut archive = tar::Archive::new(decoder);
        let entries = archive
            .entries()
            .map_err(|error| format!("cannot read release archive: {error}"))?;
        let mut held_entry = false;
        // What the volume can still take once the files read so far land.
        let mut bytes_left = room.bytes;
        for (index, entry) in entries.enumerate() {
            held_entry = true;
            // `index` entries came before this one, so this entry is the
            // `index + 1`th: more than the volume holds once `index` reaches it.
            let before = u64::try_from(index)
                .map_err(|error| format!("release entry count does not fit u64: {error}"))?;
            if before >= room.entries {
                return Err(format!(
                    "release archive holds more entries than the {} files {} can still hold",
                    room.entries,
                    parent.display()
                ));
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
                let size = entry.header().size().map_err(|error| {
                    format!("invalid release entry size for {}: {error}", path.display())
                })?;
                bytes_left = bytes_left.checked_sub(size).ok_or_else(|| {
                    format!(
                        "release archive expands beyond the {} bytes {} can still hold",
                        room.bytes,
                        parent.display()
                    )
                })?;
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
                let archived_mode = entry.header().mode().map_err(|error| {
                    format!("invalid release entry mode for {}: {error}", path.display())
                })?;
                let source_mode = nix::libc::mode_t::try_from(archived_mode)
                    .map(Mode::from_bits_truncate)
                    .map_err(|error| {
                        format!(
                            "release entry mode {archived_mode:o} for {} is not a file mode: {error}",
                            path.display()
                        )
                    })?;
                let executable =
                    source_mode.intersects(Mode::S_IXUSR | Mode::S_IXGRP | Mode::S_IXOTH);
                let owner_only = !source_mode.intersects(Mode::S_IRWXG | Mode::S_IRWXO);
                // An executable keeps its sharing; owner-only stays owner-only;
                // a data file is the owner's alone.
                let mode = match (executable, owner_only) {
                    (true, true) => Mode::S_IRWXU,
                    (true, false) => {
                        Mode::S_IRWXU
                            | Mode::S_IRGRP
                            | Mode::S_IXGRP
                            | Mode::S_IROTH
                            | Mode::S_IXOTH
                    }
                    (false, _) => Mode::S_IRUSR | Mode::S_IWUSR,
                };
                std::fs::set_permissions(
                    &output,
                    std::fs::Permissions::from_mode(u32::from(mode.bits())),
                )
                .map_err(|error| {
                    format!("cannot set release mode {}: {error}", output.display())
                })?;
            }
        }
        if !held_entry {
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
    result.map_err(|error| match std::fs::remove_dir_all(&staging) {
        Ok(()) => error,
        Err(removal) if removal.kind() == std::io::ErrorKind::NotFound => error,
        Err(removal) => format!(
            "{error}; the partly extracted release {} is left behind: {removal}",
            staging.display()
        ),
    })
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
