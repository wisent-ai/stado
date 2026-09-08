//! The owned work files a machine submission stages its source archive
//! through, plus the size limits and path rules that bound them.

use std::io::Read;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

pub(in crate::machine) mod archive;
pub(in crate::machine) mod staging;

pub const MAX_SOURCE_ARCHIVE_BYTES: u64 = 512 * 1024 * 1024;
pub const MAX_SOURCE_EXTRACTED_BYTES: u64 = 2 * 1024 * 1024 * 1024;
pub const MAX_SOURCE_MEMBERS: u64 = 100_000;

/// Validate one archive entry name against the Python path rules:
/// non-empty, no backslashes, not absolute, no `..`/empty/`.` components.
pub(in crate::machine) fn unsafe_archive_name(name: &str) -> bool {
    name.is_empty()
        || name.contains('\\')
        || name.starts_with('/')
        || name
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

pub(in crate::machine) struct OwnedMachineFile {
    path: Option<PathBuf>,
}

impl OwnedMachineFile {
    pub(in crate::machine) fn path(&self) -> &Path {
        self.path.as_deref().expect("owned machine file is live")
    }

    pub(in crate::machine) fn cleanup(mut self) -> Result<(), std::io::Error> {
        if let Some(path) = self.path.take() {
            std::fs::remove_file(&path)?;
            if let Some(parent) = path.parent() {
                std::fs::File::open(parent)?.sync_all()?;
            }
        }
        Ok(())
    }
}

impl Drop for OwnedMachineFile {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = std::fs::remove_file(path);
        }
    }
}

fn machine_source_work_root() -> Result<PathBuf, std::io::Error> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::NotFound, "HOME is not set"))?;
    let root = PathBuf::from(home)
        .join(".stado")
        .join("work")
        .join("stado")
        .join("machine-sources");
    std::fs::create_dir_all(&root)?;
    let metadata = std::fs::symlink_metadata(&root)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::other(
            "machine source work root must be a real directory",
        ));
    }
    Ok(root)
}

fn create_owned_machine_file(
    purpose: &str,
) -> Result<(OwnedMachineFile, std::fs::File), std::io::Error> {
    let root = machine_source_work_root()?;
    for _ in 0..16 {
        let path = root.join(format!("{purpose}-{}", uuid::Uuid::new_v4().simple()));
        let mut options = std::fs::OpenOptions::new();
        options.read(true).write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        match options.open(&path) {
            Ok(file) => {
                std::fs::File::open(&root)?.sync_all()?;
                return Ok((OwnedMachineFile { path: Some(path) }, file));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(error),
        }
    }
    Err(std::io::Error::new(
        std::io::ErrorKind::AlreadyExists,
        "could not reserve a unique machine source work file",
    ))
}

pub(in crate::machine) fn sha256_path(path: &Path) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut digest = Sha256::new();
    let mut chunk = [0u8; 1024 * 1024];
    loop {
        let n = file.read(&mut chunk)?;
        if n == 0 {
            break;
        }
        digest.update(&chunk[..n]);
    }
    Ok(hex::encode(digest.finalize()))
}
