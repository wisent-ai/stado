//! What the delivery routine writes in Oko's defect register, through Oko's
//! own CLI: one defect per authoring session for a refused step, carrying
//! every refusal of the run; a note on those defects when the next revision
//! still refuses; and the repair on them once a revision installs.

use std::path::Path;

use serde::Deserialize;

use super::state::Refused;
use super::{process, succeeds};
use crate::cli::CmdError;

/// Oko's program; the routine records in Oko through its CLI only.
const OKO: &str = "oko";

#[derive(Deserialize)]
struct Listed {
    id: String,
    #[serde(default)]
    repaired_in: Option<String>,
    #[serde(default)]
    owner: Option<String>,
}

#[derive(Deserialize)]
struct Recorded {
    id: String,
}

/// The sessions owning the open defects whose recorded repair `revision` of
/// `checkout` carries and `since` (the revision last delivered, when there is
/// one) does not: the authors of the work the refused revision brought.
pub async fn authors(
    checkout: &Path,
    repository: &str,
    since: Option<&str>,
    revision: &str,
) -> Result<Vec<String>, CmdError> {
    let listed = process(OKO, &["defects", "list", "--open", "--json"], None).await?;
    let defects: Vec<Listed> = serde_json::from_str(&listed).map_err(|error| {
        CmdError::click(format!(
            "oko defects list --open --json answered no defect list: {error}"
        ))
    })?;
    let mut authors: Vec<String> = Vec::new();
    for defect in defects {
        let (Some(repaired_in), Some(owner)) = (defect.repaired_in, defect.owner) else {
            continue;
        };
        let Some((named, commit)) = repaired_in.rsplit_once('@') else {
            continue;
        };
        if named != repository || authors.contains(&owner) {
            continue;
        }
        let carried = succeeds(
            "git",
            &["merge-base", "--is-ancestor", commit, revision],
            checkout,
        )
        .await?;
        let landed = match since {
            Some(since) => {
                succeeds(
                    "git",
                    &["merge-base", "--is-ancestor", commit, since],
                    checkout,
                )
                .await?
            }
            None => false,
        };
        if carried && !landed {
            authors.push(owner);
        }
    }
    Ok(authors)
}

/// Record `refused` once per author (once with no owner when the revision
/// carries no recorded repair), each defect carrying every refusal; their ids.
pub async fn record(
    product: &str,
    repository: &str,
    refused: &Refused,
    authors: &[String],
) -> Result<Vec<String>, CmdError> {
    let title = format!(
        "The delivery routine's install of {} at {}@{} refused, with {} refusal(s) in one run",
        refused.step,
        repository,
        refused.revision,
        refused.refusals.len()
    );
    let surface = format!(
        "stado ci run, step {} (stado product install at {repository}@{})",
        refused.step, refused.revision
    );
    let evidence = format!(
        "Every refusal of the run, each gate run to its end:\n{}",
        refused.refusals.join("\n")
    );
    let found = format!("{repository}@{}", refused.revision);
    let owners: Vec<Option<&String>> = match authors.is_empty() {
        true => vec![None],
        false => authors.iter().map(Some).collect(),
    };
    let mut ids = Vec::with_capacity(owners.len());
    for owner in owners {
        let answer = process(
            OKO,
            &[
                "defects",
                "record",
                "--product",
                product,
                "--title",
                &title,
                "--surface",
                &surface,
                "--evidence",
                &evidence,
                "--impact",
                "The delivery routine stops at this step: it and every later step of the plan stay uninstalled until a commit on main repairs every refusal",
                "--repair",
                "Repair every refusal named in the evidence in the source it names and commit it to main; the next pass of stado ci run resumes at this step",
                "--found-in",
                &found,
                "--json",
            ],
            None,
        )
        .await?;
        let id = serde_json::from_str::<Recorded>(&answer)
            .map_err(|error| {
                CmdError::click(format!(
                    "oko defects record answered no recorded defect: {error}: {answer}"
                ))
            })?
            .id;
        if let Some(owner) = owner {
            process(
                OKO,
                &[
                    "defects",
                    "assign",
                    &id,
                    "--owner",
                    owner.as_str(),
                    "--priority",
                    "high",
                ],
                None,
            )
            .await?;
        }
        ids.push(id);
    }
    Ok(ids)
}

/// Note on `defects` that the step refused again at a later revision.
pub async fn refused_again(defects: &[String], refused: &Refused) -> Result<(), CmdError> {
    let text = format!(
        "The delivery routine's install of {} still refused at {}, every gate run to its end:\n{}",
        refused.step,
        refused.revision,
        refused.refusals.join("\n")
    );
    for defect in defects {
        process(OKO, &["defects", "note", defect, "--text", &text], None).await?;
    }
    Ok(())
}

/// Record on `defects` that `repository`@`revision` installed the step.
pub async fn repaired(
    defects: &[String],
    step: &str,
    repository: &str,
    revision: &str,
) -> Result<(), CmdError> {
    let revision = format!("{repository}@{revision}");
    let detail = format!("The delivery routine installed {step} from {revision}");
    for defect in defects {
        process(
            OKO,
            &[
                "defects",
                "repaired",
                defect,
                "--revision",
                &revision,
                "--detail",
                &detail,
            ],
            None,
        )
        .await?;
    }
    Ok(())
}
