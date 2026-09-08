//! Bounding owner-written service logs while the host has ample space.

use std::fs::OpenOptions;
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use crate::providers::local::disk_cleanup::janitor::{ifmt, IFREG};

const SERVICE_LOG_MAX_BYTES: u64 = 64 * 1024 * 1024;
const SERVICE_LOG_KEEP_BYTES: u64 = 4 * 1024 * 1024;
const SERVICE_LOG_SCAN_LIMIT: usize = 512;

/// Bound owner-written service logs even while the host has ample free space.
///
/// launchd appends forever to `StandardOutPath` and `StandardErrorPath`; disk
/// pressure is too late to enforce a per-file bound. Keep the newest 4 MiB in
/// place so an already-open `O_APPEND` descriptor continues writing the same
/// inode. Symlinks, hard links, foreign owners, and non-log files are refused.
pub(crate) fn rotate_service_logs(home: &Path, log_fn: &mut dyn FnMut(&str)) {
    let root = home.join(".stado").join("logs");
    let entries = match std::fs::read_dir(&root) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return,
        Err(error) => {
            log_fn(&format!("service log scan failed: {error}"));
            return;
        }
    };
    let owner = unsafe { nix::libc::geteuid() };
    for entry in entries.take(SERVICE_LOG_SCAN_LIMIT) {
        let entry = match entry {
            Ok(entry) => entry,
            Err(error) => {
                log_fn(&format!("service log entry unreadable: {error}"));
                continue;
            }
        };
        let path = entry.path();
        if !matches!(
            path.extension().and_then(|value| value.to_str()),
            Some("log" | "out" | "err")
        ) {
            continue;
        }
        let metadata = match entry.metadata() {
            Ok(metadata)
                if ifmt(metadata.mode()) == IFREG
                    && metadata.uid() == owner
                    && metadata.nlink() == 1
                    && metadata.len() > SERVICE_LOG_MAX_BYTES =>
            {
                metadata
            }
            Ok(_) => continue,
            Err(error) => {
                log_fn(&format!("service log metadata unreadable: {error}"));
                continue;
            }
        };
        let result = (|| -> io::Result<u64> {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .open(&path)?;
            let opened = file.metadata()?;
            if ifmt(opened.mode()) != IFREG
                || opened.uid() != owner
                || opened.nlink() != 1
                || opened.len() <= SERVICE_LOG_MAX_BYTES
            {
                return Ok(opened.len());
            }
            let start = opened.len().saturating_sub(SERVICE_LOG_KEEP_BYTES);
            file.seek(SeekFrom::Start(start))?;
            let mut tail = Vec::with_capacity((opened.len() - start) as usize);
            (&mut file)
                .take(SERVICE_LOG_KEEP_BYTES)
                .read_to_end(&mut tail)?;
            file.seek(SeekFrom::Start(0))?;
            file.write_all(&tail)?;
            file.set_len(tail.len() as u64)?;
            file.sync_data()?;
            Ok(tail.len() as u64)
        })();
        match result {
            Ok(after) if after < metadata.len() => log_fn(&format!(
                "service log rotated file={} bytes_before={} bytes_after={after}",
                entry.file_name().to_string_lossy(),
                metadata.len()
            )),
            Ok(_) => {}
            Err(error) => log_fn(&format!(
                "service log rotation failed file={}: {error}",
                entry.file_name().to_string_lossy()
            )),
        }
    }
}
