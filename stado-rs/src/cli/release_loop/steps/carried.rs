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

use serde::{Deserialize, Serialize};

use super::process::{run, succeeds};
use crate::cli::release_newest::{Planned, Standing};
use crate::cli::CmdError;
use crate::release_pipeline::PRODUCT_MANIFEST;

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
    /// The tasks Oko closed on the same release: every acceptance check met
    /// and their evidence commit carried.
    #[serde(default)]
    tasks: Vec<ReleasedTask>,
    #[serde(default)]
    unreadable: Vec<String>,
}

#[derive(Deserialize)]
struct ReleasedDefect {
    id: String,
    closed: bool,
}

#[derive(Deserialize)]
struct ReleasedTask {
    task: String,
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

/// An open defect's recorded repair that no checkout of the plan carries,
/// and why: a repair the queue does not know is a release nobody will cut.
#[derive(Debug, Clone, Serialize)]
pub struct Unplaced {
    pub defect: String,
    pub revision: String,
    pub why: String,
}

/// The commit a planned checkout stands on, or why it has none to release.
fn standing_commit(standing: &Standing) -> Result<&str, String> {
    match standing {
        Standing::Releasable { commit, .. }
        | Standing::Published { commit, .. }
        | Standing::InFlight { commit, .. }
        | Standing::VersionTaken { commit, .. } => Ok(commit),
        Standing::DeclaresNoReleases { reason } => Err(format!("declares no releases: {reason}")),
        Standing::Unreadable { refusal } => Err(format!("could not be read: {refusal}")),
    }
}

/// Every repair in `repairs` that no checkout of `planned` (the plan of
/// `root`) carries, each with why: the workspace holds no checkout of the
/// repository, or one that declares no product release; the checkout
/// declares no releases or cannot be read; it holds no such commit (the
/// history the repair was recorded in was rewritten, or never reached it);
/// or the commit is not in the history the checkout stands on.
pub async fn unplaced(
    root: &Path,
    planned: &[Planned],
    repairs: &[Repair],
) -> Result<Vec<Unplaced>, CmdError> {
    let mut unplaced = Vec::new();
    for repair in repairs {
        let revision = format!("{}@{}", repair.repository, repair.commit);
        let checkout = planned.iter().find(|planned| {
            root.join(&planned.checkout)
                .file_name()
                .and_then(|name| name.to_str())
                == Some(repair.repository.as_str())
        });
        let why = match checkout {
            None => match root.join(&repair.repository).is_dir() {
                true => format!(
                    "the workspace's checkout {} declares no product release ({PRODUCT_MANIFEST}), so no release carries it",
                    repair.repository
                ),
                false => format!("{} holds no checkout of {}", root.display(), repair.repository),
            },
            Some(planned) => {
                let directory = root.join(&planned.checkout);
                match standing_commit(&planned.standing) {
                    Err(why) => format!("{} {why}", repair.repository),
                    Ok(stands) => {
                        let object = format!("{}^{{commit}}", repair.commit);
                        if !succeeds("git", &["cat-file", "-e", &object], &directory).await? {
                            format!(
                                "{} holds no commit {}: the history the repair was recorded in was rewritten or never reached this checkout",
                                repair.repository, repair.commit
                            )
                        } else if !succeeds(
                            "git",
                            &["merge-base", "--is-ancestor", &repair.commit, stands],
                            &directory,
                        )
                        .await?
                        {
                            format!(
                                "{} is not in the history of {stands}, the commit {} stands on; `oko defects rebased --apply` records the commit that carries the same patch",
                                repair.commit, repair.repository
                            )
                        } else {
                            continue;
                        }
                    }
                }
            }
        };
        unplaced.push(Unplaced {
            defect: repair.id.clone(),
            revision,
            why,
        });
    }
    Ok(unplaced)
}

/// Close every open defect a published release of `product` now carries,
/// and every task it finishes, and answer their ids (the tasks' are
/// `task-…`). Oko names every repository or product whose releases it could
/// not read; only one that is this checkout's repository or this product
/// fails the step, since the others are not this release.
pub async fn record_released(product: &str, repository: &str) -> Result<Vec<String>, CmdError> {
    let answer = run(
        OKO,
        &[
            "defects",
            "released",
            "--product",
            product,
            "--close",
            "--tasks",
            "--json",
        ],
        None,
    )
    .await?;
    let released: Released = serde_json::from_str(&answer).map_err(|error| {
        CmdError::click(format!(
            "oko defects released --product {product} --close --tasks --json answered no report: {error}"
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
    let defects = released
        .carried
        .into_iter()
        .filter(|defect| defect.closed)
        .map(|defect| defect.id);
    let tasks = released
        .tasks
        .into_iter()
        .filter(|task| task.closed)
        .map(|task| task.task);
    Ok(defects.chain(tasks).collect())
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
