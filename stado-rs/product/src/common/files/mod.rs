mod locks;
mod record;

use anyhow::{bail, Context, Result};
pub(crate) use locks::lock_surface_waiting;
pub use locks::{
    alive, lock, lock_superseding, lock_waiting, mark_placed, mark_placing, mark_step,
    PHASE_PLACING,
};
pub use record::locks_report;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

pub(crate) fn read_lock_record(path: &Path) -> Result<Option<record::Recorded>> {
    record::read(path, Some(record::product(path)?))
}

/// The first line of a `CACHEDIR.TAG` that declares its directory
/// regenerable, by the Cache Directory Tagging Specification
/// (https://bford.info/cachedir/).
pub const CACHEDIR_SIGNATURE: &str = "Signature: 8a477f597d28d172789f06886806bc55";

/// Mark `directory` as a tree its build tool regenerates, so the janitor's
/// build-cache cleaner may reclaim it whole under disk pressure. `writer`
/// names who wrote the tag, for a reader of the file.
pub fn tag_cache(directory: &Path, writer: &str) -> Result<()> {
    fs::create_dir_all(directory).with_context(|| format!("creating {}", directory.display()))?;
    fs::write(
        directory.join("CACHEDIR.TAG"),
        format!(
            "{CACHEDIR_SIGNATURE}\n# Written by {writer}: the next build reproduces this tree.\n"
        ),
    )
    .with_context(|| format!("tagging {}", directory.display()))
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().context("output has no parent directory")?;
    fs::create_dir_all(parent)?;
    if path
        .symlink_metadata()
        .is_ok_and(|m| m.file_type().is_symlink())
    {
        bail!("refusing symlinked output {}", path.display());
    }
    let temporary = parent.join(format!(
        ".{}-{}.tmp",
        path.file_name()
            .context("missing filename")?
            .to_string_lossy(),
        uuid::Uuid::new_v4()
    ));
    let result = (|| {
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&temporary)?;
        if let Ok(metadata) = path.metadata() {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hash = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        let length = file.read(&mut buffer)?;
        if length == 0 {
            break;
        }
        hash.update(&buffer[..length]);
    }
    Ok(hex::encode(hash.finalize()))
}
