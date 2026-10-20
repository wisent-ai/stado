mod locks;
mod presence;
mod record;

use anyhow::{bail, Context, Result};
pub(crate) use locks::lock_surface_waiting;
pub use locks::{
    alive, lock, lock_superseding, lock_waiting, mark_placed, mark_placing, mark_step,
    PHASE_PLACING,
};
pub use presence::{present, remove_tree};
pub use record::locks_report;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::Write,
    path::Path,
};

/// Free bytes the filesystem reports at `path`, shared by the installer and
/// its build-tree reclaimer.
pub fn available_space(path: &Path) -> Result<u64> {
    // statvfs(3) through nix answers its own errno, so the call's return
    // value is never compared by hand.
    let state = nix::sys::statvfs::statvfs(path)
        .with_context(|| format!("reading free space under {}", path.display()))?;
    // fsblkcnt_t is u32 on macOS and u64 on Linux, and f_frsize is c_ulong:
    // the conversions are real on one and the identity on the other.
    #[allow(clippy::useless_conversion)]
    let (blocks, size) = (
        u64::from(state.blocks_available()),
        u64::from(state.fragment_size()),
    );
    Ok(blocks.saturating_mul(size))
}

pub(crate) fn read_lock_record(path: &Path) -> Result<Option<record::Recorded>> {
    let expected = record::product(path)?;
    let recorded = record::read(path, Some(expected))?;
    if let Some(recorded) = &recorded {
        recorded.validate_product(path, expected)?;
    }
    Ok(recorded)
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
    // An output that does not exist yet is written new; one that cannot be
    // examined is refused rather than taken for absent.
    let existing = match path.symlink_metadata() {
        Ok(metadata) => Some(metadata),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => {
            return Err(error).with_context(|| format!("examining output {}", path.display()))
        }
    };
    if existing
        .as_ref()
        .is_some_and(|metadata| metadata.file_type().is_symlink())
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
            // A new output is its owner's alone until it takes the mode of
            // the file it replaces.
            options.mode(u32::from(libc::S_IRUSR | libc::S_IWUSR));
        }
        let mut file = options.open(&temporary)?;
        if let Some(metadata) = &existing {
            file.set_permissions(metadata.permissions())?;
        }
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    let Err(error) = result else {
        return Ok(());
    };
    // The write failed; the temporary file it leaves, if any, is said.
    match fs::remove_file(&temporary) {
        Ok(()) => Err(error),
        Err(removal) if removal.kind() == std::io::ErrorKind::NotFound => Err(error),
        Err(removal) => Err(error.context(format!(
            "the temporary file {} stays: {removal}",
            temporary.display()
        ))),
    }
}

pub fn atomic_json(path: &Path, value: &Value) -> Result<()> {
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    atomic_write(path, &bytes)
}

pub fn sha256(path: &Path) -> Result<String> {
    let mut file = File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut hash = Sha256::new();
    std::io::copy(&mut file, &mut hash).with_context(|| format!("reading {}", path.display()))?;
    Ok(hex::encode(hash.finalize()))
}
