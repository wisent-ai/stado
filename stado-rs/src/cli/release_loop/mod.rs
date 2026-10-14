//! The delivery queue: every commit that carries a recorded repair waits,
//! named with its defects and the steps it needs, for the release the build
//! gate admits.
//!
//! A repair recorded in Oko (`oko defects repaired`) used to stop at the
//! commit, with nothing saying which commit owed which release, so every
//! session met again the failure its source already fixed. For each product
//! checkout of the workspace the queue reads the commit the checkout stands
//! on and Oko's open defects whose recorded repair that commit carries. With
//! none, nothing is owed (`nothing_carried`). With some, the commit is
//! `queued` at the first step it needs: `version` when its declared version
//! is taken (the least next version the versioning rule allows), `release`
//! (the pipeline `stado release submit` drives — build, the product's own
//! quality steps, signing, publication and delivery to the hosts that run
//! the product), or `record` when a release already carries it (the release
//! recorded on its defects, which closes them).
//!
//! Queueing builds nothing and commits nothing. `stado serve
//! --release-loop-root DIR` queues at start and whenever a checkout's
//! branch moves, and `stado release newest --plan` queues and shows it.
//! The steps run only in `stado release newest`, the release command the
//! build gate admits or refuses like every other build: a step that refuses
//! keeps its exact error on the commit and is noted on every carried defect,
//! and the commit is not taken again; the product's next commit is.

pub mod push;
pub mod register;
mod steps;
pub mod watch;

use std::path::Path;

use crate::cli::release_newest::Planned;
use crate::cli::release_submit::SubmitChannel;
use crate::cli::CmdError;

use register::{Entry, State};
pub use steps::Unplaced;

/// Queue every commit of `planned` that carries a repair; build nothing.
pub async fn queue(root: &Path, planned: &[Planned]) -> Result<Vec<Entry>, CmdError> {
    steps::pass(root, planned.to_vec(), steps::Mode::Queue).await
}

/// Every open defect's recorded repair that no checkout of `planned`
/// carries, with why, read from Oko's open defects; a repair the queue
/// does not know is a release nobody would cut.
pub async fn unplaced(root: &Path, planned: &[Planned]) -> Result<Vec<Unplaced>, CmdError> {
    let repairs = steps::open_repairs().await?;
    steps::unplaced(root, planned, &repairs).await
}

/// Run the queued steps of every commit of `planned` on `channel`, and
/// answer each entry; the caller is the gated release command.
pub async fn deliver(
    root: &Path,
    planned: &[Planned],
    channel: SubmitChannel,
) -> Result<Vec<Entry>, CmdError> {
    steps::pass(root, planned.to_vec(), steps::Mode::Deliver(channel)).await
}
/// The products whose commit the queue delivered, or failed at a step that
/// acted, in `entries`: `release newest` does not submit them again.
pub fn handled(entries: &[Entry]) -> Vec<String> {
    entries
        .iter()
        .filter(|entry| match entry.state {
            State::Delivered => true,
            State::Failed => matches!(
                entry.step,
                register::Step::Gates
                    | register::Step::Version
                    | register::Step::Release
                    | register::Step::Record
            ),
            State::Running | State::Queued | State::NothingCarried => false,
        })
        .map(|entry| entry.product.clone())
        .collect()
}

/// One line per entry, as `stado release newest` prints the queue.
pub fn print(entries: &[Entry]) {
    for entry in entries {
        println!("  queue  {}", register::line(entry));
    }
}
