//! The delivery loop as a role of `stado serve`: one pass at start, and
//! another each time a branch of a product checkout under the root moves —
//! a commit, a merge, a pull. Nothing is polled on an interval: the kernel's
//! change notification on every checkout's `.git/refs/heads` wakes the role,
//! armed before each pass so a commit that lands during a pass still wakes
//! the next one. A pass that refuses as a whole (the workspace cannot be
//! planned) is said on stderr and the role waits for the next move; each
//! product's own failure is already kept in the register and on its defects.

use std::path::{Path, PathBuf};

use super::register;
use super::steps::pass;
use crate::cli::release_submit::SubmitChannel;
use crate::cli::CmdError;
use crate::release_pipeline::PRODUCT_MANIFEST;

/// The directory a checkout's branch heads are written in.
fn heads(checkout: &Path) -> PathBuf {
    checkout.join(".git").join("refs").join("heads")
}

/// The branch-head directories of every product checkout directly under
/// `root`.
fn watched(root: &Path) -> Result<Vec<PathBuf>, CmdError> {
    let listing = std::fs::read_dir(root)
        .map_err(|error| CmdError::click(format!("listing {}: {error}", root.display())))?;
    let mut directories = Vec::new();
    for entry in listing {
        let entry = entry
            .map_err(|error| CmdError::click(format!("listing {}: {error}", root.display())))?;
        let checkout = entry.path();
        if checkout.join(PRODUCT_MANIFEST).is_file() && heads(&checkout).is_dir() {
            directories.push(heads(&checkout));
        }
    }
    if directories.is_empty() {
        return Err(CmdError::click(format!(
            "{} holds no product checkout with a {PRODUCT_MANIFEST} and a .git/refs/heads to watch",
            root.display()
        )));
    }
    Ok(directories)
}

/// `stado serve --release-loop-root ROOT --release-loop-channel CHANNEL`.
pub async fn role(root: PathBuf, channel: SubmitChannel) -> Result<(), CmdError> {
    loop {
        let directories = watched(&root)?;
        let mut watch = crate::queue::local_file::watch_directories(&directories)
            .map_err(|error| CmdError::click(format!("watching {}: {error}", root.display())))?;
        match pass(&root, &[], channel).await {
            Ok(entries) => {
                for entry in &entries {
                    eprintln!("stado release loop: {}", register::line(entry));
                }
            }
            Err(error) => eprintln!(
                "stado release loop: the pass over {} refused: {error}",
                root.display()
            ),
        }
        tokio::task::spawn_blocking(move || watch.next())
            .await
            .map_err(|error| {
                CmdError::click(format!(
                    "the branch watch over {} ended: {error}",
                    root.display()
                ))
            })?
            .map_err(|error| {
                CmdError::click(format!(
                    "the branch watch over {} failed: {error}",
                    root.display()
                ))
            })?;
    }
}
