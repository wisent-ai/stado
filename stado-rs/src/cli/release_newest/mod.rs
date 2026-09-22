//! `stado release newest` — release every product in this workspace from the
//! commit and the version it already declares.
//!
//! Releasing used to be one `stado release submit --source <path> --version
//! <version>` per product, and the caller had to know each checkout's path
//! and repeat a version the pipeline was about to read for itself: `submit`
//! reads `version_source` out of the committed manifest and refuses a
//! `--version` that disagrees with it. So the argument carried no information
//! and every product needed its own line.
//!
//! This walks the workspace, and for every product checkout it finds it reads
//! the commit that checkout is on, reads the version that commit declares,
//! skips what is already published at that version, and submits the rest
//! through the same pipeline `submit` drives. Nothing here decides a version:
//! the rule that says which slot advances lives once for the whole fleet in
//! AutoVersion, and the number it produced is already committed in the
//! product's own source by the time a release is cut.

mod plan;
mod report;

use std::path::PathBuf;

use clap::Args;

use crate::cli::release_submit::SubmitChannel;
use crate::cli::CmdError;

pub use plan::{plan, Planned, Standing};
use report::print_plan;

#[derive(Args)]
pub struct ReleaseNewestArgs {
    /// The directory holding the product checkouts. Defaults to the parent of
    /// the checkout this command is run in, which is where a workspace keeps
    /// one checkout per repository.
    #[arg(long)]
    root: Option<PathBuf>,
    /// Release only these products; repeat for several. The default is every
    /// product the workspace holds.
    #[arg(long = "product")]
    products: Vec<String>,
    #[arg(long, value_enum, default_value_t = SubmitChannel::Candidate)]
    channel: SubmitChannel,
    /// Read what would be released and why the rest is skipped, without
    /// submitting anything.
    #[arg(long)]
    plan: bool,
    #[arg(long)]
    json: bool,
}

pub async fn newest(args: &ReleaseNewestArgs) -> Result<(), CmdError> {
    let root = workspace(args.root.clone())?;
    let planned = plan(&root, &args.products).await?;
    if args.plan {
        print_plan(&root, &planned, args.json);
        return Ok(());
    }
    report::submit_planned(&root, planned, args.channel, args.json).await
}

/// Where the product checkouts are.
///
/// A workspace keeps one checkout per repository side by side, so the parent
/// of the checkout the operator is standing in is the workspace. Outside any
/// checkout there is nothing to infer and the refusal says which flag to pass
/// rather than guessing a directory and releasing whatever is under it.
fn workspace(requested: Option<PathBuf>) -> Result<PathBuf, CmdError> {
    if let Some(root) = requested {
        return root
            .canonicalize()
            .map_err(|error| CmdError::click(format!("--root {}: {error}", root.display())));
    }
    let here = std::env::current_dir()?;
    let checkout = crate::binary::provenance::checkout_root(&here).ok_or_else(|| {
        CmdError::click(format!(
            "{} is not inside a product checkout, so the workspace holding the \
             checkouts cannot be read from it; name it with --root <DIR>",
            here.display()
        ))
    })?;
    let parent = checkout.parent().ok_or_else(|| {
        CmdError::click(format!(
            "{} has no parent directory to read the workspace from; name it \
             with --root <DIR>",
            checkout.display()
        ))
    })?;
    Ok(parent.to_path_buf())
}
