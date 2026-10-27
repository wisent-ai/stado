//! A drain asked of one worker: claim nothing more, let the jobs it runs
//! finish, and leave its loop once they have.
//!
//! The request is a file in the worker's own home, `~/.stado/drain`, so
//! whoever started the worker can ask it of exactly that worker and of no
//! other on the host: a lending session started for a peer fleet runs its
//! worker with a home of its own, and withdraws it by creating this file
//! there. The file's contents are the reason, which the worker says once.
//! Ending the process instead would leave the jobs it started running in
//! their own process groups with no worker renewing their leases, so their
//! queue would hand them out again while they still ran here.

use std::path::PathBuf;

/// The drain request of the worker whose home this process has.
pub fn path() -> PathBuf {
    crate::config_file::expand_tilde("~/.stado/drain")
}

/// The reason a drain was asked for, when one was. A request that exists and
/// cannot be read is still a request: the read error is its reason.
pub fn requested() -> Option<String> {
    let path = path();
    match std::fs::read_to_string(&path) {
        Ok(reason) => Some(reason.trim().to_string()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => Some(format!(
            "a drain request at {} that cannot be read: {error}",
            path.display()
        )),
    }
}

/// Ask the worker whose home is `home` to drain, for `reason`.
pub fn request(home: &std::path::Path, reason: &str) -> std::io::Result<()> {
    let path = home.join(".stado").join("drain");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, reason)
}
