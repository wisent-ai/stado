//! The delivery queue as a role of `stado serve`: one pass at start, and
//! another each time a branch of a product checkout under the root moves —
//! a commit, a merge, a pull — or the checkout's view of its origin moves (a
//! push to the delivery branch). Nothing is polled on an interval: the
//! kernel's change notification on every checkout's `.git/refs/heads` and
//! `.git/refs/remotes/origin` wakes the role, armed before each pass so a
//! commit that lands during a pass still wakes the next one. Each pass
//! queues the release loop's commits, then runs the main delivery queue's
//! builder over every checkout ([`crate::cli::ci::queue`]): the one place a
//! candidate on the delivery branch is gated and `main` moved. A pass that
//! refuses as a whole (the workspace cannot be planned) is said on stderr
//! and the role waits for the next move.

use std::path::{Path, PathBuf};

use super::{queue, register};
use crate::cli::ci::queue as delivery;
use crate::cli::release_newest::plan;
use crate::cli::CmdError;
use crate::release_pipeline::PRODUCT_MANIFEST;

/// The directory a checkout's branch heads are written in.
fn heads(checkout: &Path) -> PathBuf {
    checkout.join(".git").join("refs").join("heads")
}

/// The directory a checkout's view of its origin's branches is written in.
fn origin(checkout: &Path) -> PathBuf {
    checkout
        .join(".git")
        .join("refs")
        .join("remotes")
        .join("origin")
}

/// Every product checkout directly under `root`.
fn checkouts(root: &Path) -> Result<Vec<PathBuf>, CmdError> {
    let listing = std::fs::read_dir(root)
        .map_err(|error| CmdError::click(format!("listing {}: {error}", root.display())))?;
    let mut checkouts = Vec::new();
    for entry in listing {
        let entry = entry
            .map_err(|error| CmdError::click(format!("listing {}: {error}", root.display())))?;
        let checkout = entry.path();
        if checkout.join(PRODUCT_MANIFEST).is_file() && heads(&checkout).is_dir() {
            checkouts.push(checkout);
        }
    }
    if checkouts.is_empty() {
        return Err(CmdError::click(format!(
            "{} holds no product checkout with a {PRODUCT_MANIFEST} and a .git/refs/heads to watch",
            root.display()
        )));
    }
    Ok(checkouts)
}

/// The branch-head and origin directories of `checkouts`.
fn watched(checkouts: &[PathBuf]) -> Vec<PathBuf> {
    let mut directories = Vec::new();
    for checkout in checkouts {
        directories.push(heads(checkout));
        if origin(checkout).is_dir() {
            directories.push(origin(checkout));
        }
    }
    directories
}

/// `stado serve --release-loop-root ROOT`.
pub async fn role(root: PathBuf) -> Result<(), CmdError> {
    loop {
        let checkouts = checkouts(&root)?;
        let mut watch = crate::queue::local_file::watch_directories(&watched(&checkouts))
            .map_err(|error| CmdError::click(format!("watching {}: {error}", root.display())))?;
        let queued = match plan(&root, &[]).await {
            Ok(planned) => queue(&root, &planned).await,
            Err(error) => Err(error),
        };
        match queued {
            Ok(entries) => {
                for entry in &entries {
                    eprintln!("stado release queue: {}", register::line(entry));
                }
            }
            Err(error) => eprintln!(
                "stado release queue: the pass over {} refused: {error}",
                root.display()
            ),
        }
        for checkout in &checkouts {
            match delivery::deliver(checkout).await {
                Ok(Some(entry)) => {
                    eprintln!("stado delivery queue: {}", delivery::register::line(&entry))
                }
                Ok(None) => {}
                Err(error) => eprintln!(
                    "stado delivery queue: the pass over {} refused: {error}",
                    checkout.display()
                ),
            }
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
