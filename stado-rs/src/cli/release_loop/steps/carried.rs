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

/// The ids of `product`'s open defects whose recorded repair `commit` of
/// `checkout` carries: the repair commit is `commit` or one of its
/// ancestors. A repair commit the checkout does not hold is not carried.
pub async fn carried(
    product: &str,
    checkout: &Path,
    commit: &str,
) -> Result<Vec<String>, CmdError> {
    let listed = run(
        OKO,
        &["defects", "list", "--open", "--product", product, "--json"],
        None,
    )
    .await?;
    let defects: Vec<Defect> = serde_json::from_str(&listed).map_err(|error| {
        CmdError::click(format!(
            "oko defects list --open --product {product} --json answered no defect list: {error}"
        ))
    })?;
    let mut carried = Vec::new();
    for defect in defects {
        let Some(revision) = defect.repaired_in else {
            continue;
        };
        let Some((repository, repair)) = revision.rsplit_once('@') else {
            continue;
        };
        let held = repository == product
            && succeeds(
                "git",
                &["merge-base", "--is-ancestor", repair, commit],
                checkout,
            )
            .await?;
        if held {
            carried.push(defect.id);
        }
    }
    Ok(carried)
}

/// Close every open defect of `product` a published release now carries,
/// and answer their ids.
pub async fn record_released(product: &str) -> Result<Vec<String>, CmdError> {
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
    if !released.unreadable.is_empty() {
        return Err(CmdError::click(format!(
            "oko defects released --product {product} could not read the releases: {}",
            released.unreadable.join("; ")
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
