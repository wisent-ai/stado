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
//! skips a version a run published or a run of another commit holds, and a
//! commit a run is still releasing, and submits the rest through the same
//! pipeline `submit` drives. Nothing here decides a version:
//! the rule that says which slot advances lives once for the whole fleet in
//! AutoVersion, and the number it produced is already committed in the
//! product's own source by the time a release is cut.

mod checkouts;
mod plan;
mod report;

use std::path::PathBuf;

use clap::Args;

use crate::cli::release_submit::SubmitChannel;
use crate::cli::CmdError;

pub use checkouts::declaring_checkouts;
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
    /// Deliver these products first, in the order given; repeat the flag or
    /// separate names with commas. The rest follow in name order. Each must
    /// be a product of the plan.
    #[arg(long = "first", value_delimiter = ',')]
    first: Vec<String>,
    #[arg(long, value_enum, default_value_t = SubmitChannel::Candidate)]
    channel: SubmitChannel,
    /// Read what would be released and why the rest is skipped, without
    /// submitting anything.
    #[arg(long)]
    plan: bool,
    /// Print the delivery queue's register as it stands — every commit the
    /// queue kept, its state and step, the defects it carried and recorded,
    /// and when its release was published and recorded — reading no
    /// checkout and running no step; the other flags but --json are not read.
    #[arg(long)]
    register: bool,
    /// After the `--first` products are delivered and installed on this
    /// host, push every checkout of the workspace holding commits its
    /// upstream does not, read each pushed product's CI, and only then
    /// deliver the rest; unpushed commits are counted per checkout before
    /// and after. Needs `--first`: the products every push needs first.
    #[arg(long)]
    push: bool,
    #[arg(long)]
    json: bool,
}

/// The workspace's plan, the delivery queue kept from it, and — without
/// `--plan` — the queue delivered and every other releasable product
/// submitted. The queue's steps run here, in the release command the build
/// gate admits, and nowhere else: `--plan` and the serve role only queue.
pub async fn newest(args: &ReleaseNewestArgs) -> Result<(), CmdError> {
    if args.register {
        return report::print_register(args.json);
    }
    let root = workspace(args.root.clone())?;
    let planned = ordered(plan(&root, &args.products).await?, &args.first)?;
    if args.plan {
        let unpushed = crate::cli::release_loop::push::unpushed(&root).await?;
        let queued = crate::cli::release_loop::queue(&root, &planned).await?;
        // With products selected, the other repositories' repairs are not
        // this plan's to place.
        let unplaced = match args.products.is_empty() {
            false => Vec::new(),
            true => match crate::cli::release_loop::unplaced(&root, &planned).await {
                Ok(unplaced) => unplaced,
                Err(error) => {
                    eprintln!("release newest: which repairs no checkout carries could not be read: {error}");
                    Vec::new()
                }
            },
        };
        print_plan(&root, &planned, &queued, &unplaced, &unpushed, args.json);
        return Ok(());
    }
    let started = chrono::Utc::now();
    let (delivered, pushing) = match args.push {
        false => (
            crate::cli::release_loop::deliver(&root, &planned, args.channel).await?,
            None,
        ),
        true => {
            if args.first.is_empty() {
                return Err(CmdError::usage(
                    "--push pushes only after the --first products are delivered and installed on this host: \
                     name with --first the products every push needs first, the one whose pre-push gate checks \
                     every push among them",
                ));
            }
            let before = crate::cli::release_loop::push::unpushed(&root).await?;
            let (leading, trailing): (Vec<Planned>, Vec<Planned>) = planned
                .iter()
                .cloned()
                .partition(|entry| args.first.contains(&entry.product));
            let mut delivered =
                crate::cli::release_loop::deliver(&root, &leading, args.channel).await?;
            let installed = crate::cli::release_loop::push::install(&leading, &delivered).await?;
            let pushed =
                crate::cli::release_loop::push::push(&root, &before, &planned, &installed).await?;
            let after = crate::cli::release_loop::push::unpushed(&root).await?;
            delivered
                .extend(crate::cli::release_loop::deliver(&root, &trailing, args.channel).await?);
            (
                delivered,
                Some(crate::cli::release_loop::push::Phase {
                    before,
                    installed,
                    pushed,
                    after,
                }),
            )
        }
    };
    let handled = crate::cli::release_loop::handled(&delivered);
    let mut planned = ordered(plan(&root, &args.products).await?, &args.first)?;
    planned.retain(|entry| !handled.contains(&entry.product));
    report::submit_planned(
        &root,
        planned,
        &delivered,
        pushing.as_ref(),
        started,
        args.channel,
        args.json,
    )
    .await
}

/// `planned` with the products `first` names moved to the front in that
/// order, the rest after them in the plan's own order. A name the plan does
/// not hold is refused: an order that silently skipped a product reads like
/// one that delivered it.
fn ordered(mut planned: Vec<Planned>, first: &[String]) -> Result<Vec<Planned>, CmdError> {
    let mut front = Vec::new();
    for wanted in first {
        let Some(at) = planned.iter().position(|entry| &entry.product == wanted) else {
            return Err(CmdError::click(format!(
                "--first {wanted} names no product of this plan; it holds {}",
                planned
                    .iter()
                    .map(|entry| entry.product.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound));
        };
        front.push(planned.remove(at));
    }
    front.extend(planned);
    Ok(front)
}

/// Where the product checkouts are.
///
/// A workspace keeps one checkout per repository side by side, so the parent
/// of the checkout the operator is standing in is the workspace. Outside any
/// checkout there is nothing to infer and the refusal says which flag to pass
/// rather than guessing a directory and releasing whatever is under it.
pub(crate) fn workspace(requested: Option<PathBuf>) -> Result<PathBuf, CmdError> {
    if let Some(root) = requested {
        return root
            .canonicalize()
            .map_err(|error| CmdError::usage(format!("--root {}: {error}", root.display())));
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
