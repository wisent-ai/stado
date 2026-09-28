use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{Read, Write},
    path::Path,
};

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

/// Take the exclusive lock at `path` and write who holds it into the file.
///
/// A refusal names that holder — its pid, whether the pid is still alive, its
/// command line and since when — because "another writer owns" alone left two
/// sessions installing Stado unable to tell a running install from a stuck one.
pub fn lock(path: &Path) -> Result<File> {
    take(path, false)
}

/// Take the exclusive lock at `path`, blocking until the current holder
/// releases it. For an operator who asked (`--wait`) to run after a
/// concurrent install of the same surface rather than be refused by it.
pub fn lock_waiting(path: &Path) -> Result<File> {
    take(path, true)
}

fn take(path: &Path, wait: bool) -> Result<File> {
    fs::create_dir_all(path.parent().context("lock has no parent")?)?;
    let mut options = OpenOptions::new();
    options.read(true).write(true).create(true).truncate(false);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600).custom_flags(libc::O_NOFOLLOW);
    }
    let mut file = options.open(path)?;
    if let Err(error) = file.try_lock_exclusive() {
        let holder = holder(path);
        if !wait {
            // The io::Error stays the source, so a caller that treats
            // WouldBlock as "busy" (the reconciliation sweep) still recognizes it.
            return Err(anyhow::Error::new(error)
                .context(format!("another writer owns {}; {holder}", path.display())));
        }
        eprintln!("waiting for {}: {holder}", path.display());
        file.lock_exclusive()
            .with_context(|| format!("waiting for {} failed", path.display()))?;
    }
    let record = serde_json::json!({
        "pid": std::process::id(),
        "command": std::env::args().collect::<Vec<_>>().join(" "),
        "acquired_at": chrono::Utc::now().to_rfc3339(),
    });
    file.set_len(0)?;
    file.write_all(format!("{record}\n").as_bytes())?;
    file.flush()?;
    Ok(file)
}

/// The holder a lock file records, as one clause for a refusal.
fn holder(path: &Path) -> String {
    let Ok(text) = fs::read_to_string(path) else {
        return "its holder record cannot be read".into();
    };
    let Ok(record) = serde_json::from_str::<Value>(text.trim()) else {
        return "the holder recorded nothing (a Stado from before holders were recorded)".into();
    };
    let pid = record["pid"].as_u64().unwrap_or_default();
    // Signal 0 delivers nothing and only asks whether the pid exists.
    let alive = i32::try_from(pid).is_ok_and(|pid| unsafe { libc::kill(pid, 0) } == 0);
    format!(
        "held by pid {pid} ({}) since {}: {}",
        if alive {
            "running"
        } else {
            "no longer running"
        },
        record["acquired_at"]
            .as_str()
            .unwrap_or("an unrecorded time"),
        record["command"]
            .as_str()
            .unwrap_or("an unrecorded command")
    )
}
