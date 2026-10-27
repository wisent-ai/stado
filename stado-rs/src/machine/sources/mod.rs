//! The owned work files a machine submission stages its source archive
//! through, plus the size limits and path rules that bound them.

#[cfg(unix)]
use nix::sys::stat::Mode;
#[cfg(unix)]
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub(in crate::machine) mod archive;
pub(crate) mod limits;
pub(in crate::machine) mod staging;

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
            if let Some(left) = crate::primitives::staging::remove(&path) {
                eprintln!("a machine source file stays: {left}");
            }
        }
    }
}

fn machine_source_work_root() -> Result<PathBuf, std::io::Error> {
    let home = crate::config_file::home().map_err(|cause| {
        std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("the machine source work root has no home: {cause}"),
        )
    })?;
    let root = home
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

/// A new owner-only file under the machine source work root. Its name ends in
/// a fresh random UUID, so a file already there under that name was not left
/// by this process; that is refused with the path rather than tried again
/// under another name.
fn create_owned_machine_file(
    purpose: &str,
) -> Result<(OwnedMachineFile, std::fs::File), std::io::Error> {
    let root = machine_source_work_root()?;
    let path = root.join(format!("{purpose}-{}", uuid::Uuid::new_v4().simple()));
    let mut options = std::fs::OpenOptions::new();
    options.read(true).write(true).create_new(true);
    #[cfg(unix)]
    options.mode(u32::from((Mode::S_IRUSR | Mode::S_IWUSR).bits()));
    let file = options.open(&path).map_err(|error| {
        std::io::Error::new(
            error.kind(),
            format!(
                "cannot create machine source work file {}: {error}",
                path.display()
            ),
        )
    })?;
    std::fs::File::open(&root)?.sync_all()?;
    Ok((OwnedMachineFile { path: Some(path) }, file))
}

pub(in crate::machine) fn sha256_path(path: &Path) -> Result<String, std::io::Error> {
    Ok(crate::primitives::digest::sha256_file(path)?.sha256)
}
