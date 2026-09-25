use crate::common::{copy_tree, sha256};
use anyhow::Result;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Serialize, Deserialize)]
pub struct Placement {
    pub source: PathBuf,
    pub destination: PathBuf,
    pub symbolic: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Prepared {
    pub placements: Vec<Placement>,
    pub source_revision: String,
    pub source_directory: Option<PathBuf>,
    pub release: Option<Value>,
}

impl Placement {
    pub fn fingerprint(&self) -> Result<Value> {
        if self.symbolic {
            return Ok(json!({"link": self.source}));
        }
        fingerprint(&self.source)
    }

    pub fn place(&self) -> Result<()> {
        if self.source == self.destination && !self.symbolic {
            self.source.symlink_metadata()?;
            return Ok(());
        }
        let parent = self
            .destination
            .parent()
            .ok_or_else(|| anyhow::anyhow!("installation destination has no parent"))?;
        fs::create_dir_all(parent)?;
        let staged = parent.join(format!(".wisent-install-{}", uuid::Uuid::new_v4()));
        let result = (|| {
            if self.symbolic {
                #[cfg(unix)]
                std::os::unix::fs::symlink(&self.source, &staged)?;
                #[cfg(not(unix))]
                anyhow::bail!("symbolic installation links require a Unix host");
            } else {
                copy_tree(&self.source, &staged)?;
            }
            if self.destination.is_dir()
                && !self
                    .destination
                    .symlink_metadata()?
                    .file_type()
                    .is_symlink()
            {
                #[cfg(target_os = "macos")]
                {
                    use std::{ffi::CString, os::unix::ffi::OsStrExt};
                    let source = CString::new(staged.as_os_str().as_bytes())?;
                    let target = CString::new(self.destination.as_os_str().as_bytes())?;
                    if unsafe {
                        libc::renamex_np(source.as_ptr(), target.as_ptr(), libc::RENAME_SWAP)
                    } != 0
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                #[cfg(target_os = "linux")]
                {
                    use std::{ffi::CString, os::unix::ffi::OsStrExt};
                    let source = CString::new(staged.as_os_str().as_bytes())?;
                    let target = CString::new(self.destination.as_os_str().as_bytes())?;
                    if unsafe {
                        libc::renameat2(
                            libc::AT_FDCWD,
                            source.as_ptr(),
                            libc::AT_FDCWD,
                            target.as_ptr(),
                            libc::RENAME_EXCHANGE,
                        )
                    } != 0
                    {
                        return Err(std::io::Error::last_os_error().into());
                    }
                }
                #[cfg(not(any(target_os = "macos", target_os = "linux")))]
                anyhow::bail!("atomic directory installation is unsupported on this platform");
            } else {
                fs::rename(&staged, &self.destination)?;
            }
            fs::File::open(parent)?.sync_all()?;
            Ok(())
        })();
        if let Ok(metadata) = staged.symlink_metadata() {
            if metadata.is_dir() && !metadata.file_type().is_symlink() {
                fs::remove_dir_all(&staged)?;
            } else {
                fs::remove_file(&staged)?;
            }
        }
        result
    }
}

fn fingerprint(path: &Path) -> Result<Value> {
    let metadata = path.symlink_metadata()?;
    if metadata.file_type().is_symlink() {
        return Ok(json!({"link": fs::read_link(path)?}));
    }
    let mut value = if metadata.is_dir() {
        let mut members = serde_json::Map::new();
        for entry in fs::read_dir(path)? {
            let entry = entry?;
            members.insert(
                entry
                    .file_name()
                    .to_str()
                    .ok_or_else(|| {
                        anyhow::anyhow!(
                            "artifact member name is not UTF-8: {}",
                            entry.path().display()
                        )
                    })?
                    .to_owned(),
                fingerprint(&entry.path())?,
            );
        }
        json!({"files": members})
    } else if metadata.is_file() {
        json!({"sha256": sha256(path)?})
    } else {
        anyhow::bail!("refusing to fingerprint special file {}", path.display());
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        value["mode"] = json!(metadata.permissions().mode() & 0o7777);
    }
    #[cfg(not(unix))]
    {
        value["readonly"] = json!(metadata.permissions().readonly());
    }
    Ok(value)
}
