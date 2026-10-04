//! Emptying owner-written service logs when the volume is full.

use std::fs::OpenOptions;
use std::io;
use std::os::unix::fs::{MetadataExt, OpenOptionsExt};
use std::path::Path;

use crate::providers::local::disk_cleanup::janitor::{ifmt, IFREG};

/// Empty every owner-written service log under `~/.stado/logs`.
///
/// Runs only in a pass the disk-full rule started: the logs are something
/// the fleet put on the host, and below the threshold a pass deletes nothing.
/// launchd appends forever to `StandardOutPath` and `StandardErrorPath`, so a
/// log is emptied in place (`set_len(0)`) and an already-open `O_APPEND`
/// descriptor keeps writing the same inode. Symlinks, hard links, foreign
/// owners, and non-log files are refused.
pub(crate) fn empty_service_logs(home: &Path, log_fn: &mut dyn FnMut(&str)) {
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
    for entry in entries {
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
                    && metadata.len() > 0 =>
            {
                metadata
            }
            Ok(_) => continue,
            Err(error) => {
                log_fn(&format!("service log metadata unreadable: {error}"));
                continue;
            }
        };
        let result = (|| -> io::Result<()> {
            let file = OpenOptions::new()
                .write(true)
                .custom_flags(nix::libc::O_NOFOLLOW)
                .open(&path)?;
            let opened = file.metadata()?;
            if ifmt(opened.mode()) != IFREG || opened.uid() != owner || opened.nlink() != 1 {
                return Ok(());
            }
            file.set_len(0)?;
            file.sync_data()
        })();
        match result {
            Ok(()) => log_fn(&format!(
                "service log emptied file={} bytes_before={}",
                entry.file_name().to_string_lossy(),
                metadata.len()
            )),
            Err(error) => log_fn(&format!(
                "service log emptying failed file={}: {error}",
                entry.file_name().to_string_lossy()
            )),
        }
    }
}
