//! The delivery loop: a commit that carries a recorded repair goes on to a
//! machine without anyone running it there.
//!
//! A repair recorded in Oko (`oko defects repaired`) used to stop at the
//! commit: nothing built it, so every session met again the failure its
//! source already fixed, and the register filled with repairs no host ran.
//! For each product checkout of the workspace the loop reads the commit the
//! checkout stands on and Oko's open defects whose recorded repair that
//! commit carries. With none, nothing is owed. With some, it commits the
//! least next version the versioning rule allows when the declared one is
//! taken, runs the release pipeline `stado release submit` drives — build,
//! the product's own quality steps, signing, publication and delivery to the
//! hosts that run the product — and records the published release on every
//! carried defect in Oko, which closes them. A step that refuses is kept
//! with its exact error on the commit, in the loop's register and as a note
//! on every carried defect.
//!
//! `stado serve --release-loop-root DIR --release-loop-channel C` runs a
//! pass at start and another whenever a checkout's branch moves
//! ([`watch::role`]); [`register`] is what the loop did with each commit.

pub mod register;
mod steps;
pub mod watch;

use std::path::Path;

use crate::cli::release_submit::SubmitChannel;
use crate::cli::CmdError;

pub use steps::pass;

/// One pass over `root` (`products` narrows it), printed one line per
/// commit, or as JSON; refused naming every product that did not reach a
/// release.
pub async fn run_pass(
    root: &Path,
    products: &[String],
    channel: SubmitChannel,
    json: bool,
) -> Result<(), CmdError> {
    let entries = pass(root, products, channel).await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        for entry in &entries {
            println!("{}", register::line(entry));
        }
    }
    let failed: Vec<&str> = entries
        .iter()
        .filter(|entry| entry.state == register::State::Failed)
        .map(|entry| entry.product.as_str())
        .collect();
    match failed.as_slice() {
        [] => Ok(()),
        failed => Err(CmdError::click(format!(
            "release loop: {} product(s) did not reach a release: {}",
            failed.len(),
            failed.join(", ")
        ))),
    }
}

/// What the loop did with each commit, newest first, `products` narrowing
/// it; one line per commit, or the register as JSON.
pub fn print_status(products: &[String], json: bool) -> Result<(), CmdError> {
    let mut document = register::read()?;
    if !products.is_empty() {
        document
            .entries
            .retain(|entry| products.contains(&entry.product));
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }
    if document.entries.is_empty() {
        println!(
            "the delivery loop has handled no commit ({})",
            register::path()?.display()
        );
    }
    for entry in &document.entries {
        println!("{}", register::line(entry));
    }
    Ok(())
}
