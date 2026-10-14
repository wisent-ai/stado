//! The defects a commit carries the repair of, and what the loop writes about
//! them in Oko's register, through Oko's own CLI.
//!
//! A repair is recorded with `oko defects repaired ID --revision` naming the
//! repository and commit that holds it; the commit the loop delivers carries
//! it when that commit is the delivered one or one of its ancestors. After
//! the release is published, `oko defects released --product P --close`
//! closes each carried defect against the published release, the same check
//! a person would run. A step that fails is noted on each carried defect with
//! the commit and its error, so the next session reads why the repair it
//! relies on is not on any host.

use std::path::Path;

use serde::Deserialize;

use super::process::{run, succeeds};
use crate::cli::CmdError;

/// Oko's program; the loop records in Oko through its CLI only.
const OKO: &str = "oko";

#[derive(Deserialize)]
struct Defect {
    id: String,
    #[serde(default)]
    repaired_in: Option<String>,
}

/// One open defect's recorded repair: the defect, the repository the
/// repair was committed in and the repair commit.
pub struct Repair {
    id: String,
    repository: String,
    commit: String,
}

/// Every open defect's recorded repair, read once for a whole pass: the
/// register holds thousands of defects, and the workspace hundreds of
/// checkouts.
pub async fn open_repairs() -> Result<Vec<Repair>, CmdError> {
    let listed = run(OKO, &["defects", "list", "--open", "--json"], None).await?;
    let defects: Vec<Defect> = serde_json::from_str(&listed).map_err(|error| {
        CmdError::click(format!(
            "oko defects list --open --json answered no defect list: {error}"
        ))
    })?;
    Ok(defects
        .into_iter()
        .filter_map(|defect| {
            let revision = defect.repaired_in?;
            let (repository, commit) = revision.rsplit_once('@')?;
            Some(Repair {
                id: defect.id,
                repository: repository.to_string(),
                commit: commit.to_string(),
            })
        })
        .collect())
}

#[derive(Deserialize)]
struct Released {
    #[serde(default)]
    carried: Vec<ReleasedDefect>,
    #[serde(default)]
    unreadable: Vec<String>,
}

#[derive(Deserialize)]
struct ReleasedDefect {
    id: String,
    closed: bool,
}

/// The ids of the open defects in `repairs` whose recorded repair `commit`
/// of `checkout` carries: the repair names this checkout's repository and
/// its commit is `commit` or one of its ancestors. A repair is recorded
/// against the repository it was committed in — the checkout's directory in
/// the workspace, as Oko resolves it — which is not always the product the
/// checkout releases (a repository can release a product of another name),
/// so the defects are matched by repository, never by the release product.
pub async fn carried(
    repairs: &[Repair],
    checkout: &Path,
    commit: &str,
) -> Result<Vec<String>, CmdError> {
    let repository = repository_of(checkout)?;
    let mut carried = Vec::new();
    for repair in repairs
        .iter()
        .filter(|repair| repair.repository == repository)
    {
        let held = succeeds(
            "git",
            &["merge-base", "--is-ancestor", &repair.commit, commit],
            checkout,
        )
        .await?;
        if held {
            carried.push(repair.id.clone());
        }
    }
    Ok(carried)
}

/// The repository `checkout` is, as Oko names it in a recorded repair.
pub fn repository_of(checkout: &Path) -> Result<&str, CmdError> {
    checkout
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} names no repository directory to match recorded repairs against",
                checkout.display()
            ))
        })
}

/// Close every open defect a published release of `product` now carries,
/// and answer their ids. Oko names every repository or product whose
/// releases it could not read; only one that is this checkout's repository
/// or this product fails the step, since the others are not this release.
pub async fn record_released(product: &str, repository: &str) -> Result<Vec<String>, CmdError> {
    let answer = run(
        OKO,
        &[
            "defects",
            "released",
            "--product",
            product,
            "--close",
            "--json",
        ],
        None,
    )
    .await?;
    let released: Released = serde_json::from_str(&answer).map_err(|error| {
        CmdError::click(format!(
            "oko defects released --product {product} --close --json answered no report: {error}"
        ))
    })?;
    let ours: Vec<&String> = released
        .unreadable
        .iter()
        .filter(|line| {
            [product, repository].iter().any(|name| {
                line.split_once(':')
                    .is_some_and(|(named, _)| named == *name)
            })
        })
        .collect();
    if !ours.is_empty() {
        return Err(CmdError::click(format!(
            "oko defects released --product {product} could not read this release: {}",
            ours.iter()
                .map(|line| line.as_str())
                .collect::<Vec<_>>()
                .join("; ")
        )));
    }
    Ok(released
        .carried
        .into_iter()
        .filter(|defect| defect.closed)
        .map(|defect| defect.id)
        .collect())
}

/// Note on every carried defect that the loop's `step` refused at `commit`.
/// A note that cannot be written is said on stderr and does not hide the
/// step's own error.
pub async fn note_failure(
    carried: &[String],
    product: &str,
    commit: &str,
    step: &str,
    error: &str,
) {
    let text = format!(
        "stado release loop: {product} at {commit} did not reach a release; the {step} step refused: {error}"
    );
    for id in carried {
        if let Err(noting) = run(OKO, &["defects", "note", id, "--text", &text], None).await {
            eprintln!("stado release loop: noting {id} in Oko: {noting}");
        }
    }
}
