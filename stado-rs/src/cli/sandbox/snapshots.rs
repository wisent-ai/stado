//! The `stado sandbox snapshot` verbs that read a sandbox's own snapshots:
//! every one it has, the one resume and fork start from, the files one
//! holds, and one file or folder taken out of it, also while the sandbox is
//! stopped. Each is one Boat Public API v1 call through the provider's
//! client.

use std::io::Write;
use std::path::Path;

use serde_json::{json, Value};

use crate::providers::BoxProvider;

use super::verbs::{failure, print};
use super::{CmdError, SnapshotGetArgs};

/// Write `bytes` into a new local file `to`, flushed before it is reported;
/// a file that exists is refused, never overwritten, naming `what` was being
/// written, the file and the reason.
pub(super) fn write_new(to: &Path, bytes: &[u8], what: &str) -> Result<(), CmdError> {
    std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(to)
        .and_then(|mut file| {
            file.write_all(bytes)?;
            file.sync_all()
        })
        .map_err(|error| {
            CmdError::click(format!("cannot write {what} to {}: {error}", to.display()))
                .stating(crate::cli::entry::error::io_failure_code(error.kind()))
        })
}

pub(super) async fn history(provider: &BoxProvider, id: &str) -> Result<(), CmdError> {
    let answer = provider
        .client
        .sandbox_snapshots(id)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn latest(provider: &BoxProvider, id: &str) -> Result<(), CmdError> {
    let answer = provider.client.latest_snapshot(id).await.map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn tree(provider: &BoxProvider, snapshot: &str) -> Result<(), CmdError> {
    let answer = provider
        .client
        .snapshot_tree(snapshot)
        .await
        .map_err(failure)?;
    print(&Value::Object(answer))
}

pub(super) async fn get(provider: &BoxProvider, args: &SnapshotGetArgs) -> Result<(), CmdError> {
    let bytes = provider
        .client
        .snapshot_file(&args.snapshot, &args.path)
        .await
        .map_err(failure)?;
    write_new(
        &args.to,
        &bytes,
        &format!("{} of snapshot {}", args.path, args.snapshot),
    )?;
    print(&json!({
        "snapshot": args.snapshot,
        "path": args.path,
        "to": args.to.display().to_string(),
        "bytes": bytes.len(),
    }))
}
