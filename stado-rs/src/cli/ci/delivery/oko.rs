//! What the delivery routine and the main check write in Oko's defect
//! register, through Oko's own CLI: one defect per authoring session for a
//! refused step or a red commit of main, carrying every refusal, its
//! revision recorded as found-in so Oko attributes it to the commit; a note
//! on those defects when the next revision still refuses; and the repair on
//! them once a revision installs.

use std::path::Path;

use serde::Deserialize;

use super::state::Refused;
use super::{process, succeeds};
use crate::cli::CmdError;

/// Oko's program; the routine records in Oko through its CLI only.
const OKO: &str = "oko";

#[derive(Deserialize)]
struct Listed {
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

/// The words of one refused finding as Oko records it, with the observable
/// result that completes its repair (`--criterion`: Oko records no defect
/// without one).
pub struct Finding {
    pub title: String,
    pub surface: String,
    pub impact: &'static str,
    pub repair: &'static str,
    pub criterion: String,
}

/// The finding a refused install of the delivery routine is.
pub fn install_refused(repository: &str, refused: &Refused) -> Finding {
    Finding {
        title: format!(
            "The delivery routine's install of {} at {}@{} refused, with {} refusal(s) in one run",
            refused.step,
            repository,
            refused.revision,
            refused.refusals.len()
        ),
        surface: format!(
            "stado ci run, step {} (stado product install at {repository}@{})",
            refused.step, refused.revision
        ),
        impact: "The delivery routine stops at this step: it and every later step of the plan stay uninstalled until a commit on main repairs every refusal",
        repair: "Repair every refusal named in the evidence in the source it names and commit it to main; the next pass of stado ci run resumes at this step",
        criterion: format!(
            "a later pass of stado ci run installs step {} from a commit of {repository} main that \
             carries the repair, and `stado ci delivery` shows no refused step",
            refused.step
        ),
    }
}

/// Record `refused` as `finding` once per author (once with no owner when
/// the revision carries no recorded repair), each defect carrying every
/// refusal and found in `repository`@ the refused revision; their ids.
pub async fn record(
    product: &str,
    repository: &str,
    finding: &Finding,
    refused: &Refused,
    authors: &[String],
) -> Result<Vec<String>, CmdError> {
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
                &finding.title,
                "--surface",
                &finding.surface,
                "--evidence",
                &evidence,
                "--impact",
                finding.impact,
                "--repair",
                finding.repair,
                "--found-in",
                &found,
                "--criterion",
                &finding.criterion,
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

/// Note on `defects` that what refused (`doing`, as "The delivery routine's
/// install of P:S") still refused at a later revision.
pub async fn refused_again(
    defects: &[String],
    doing: &str,
    refused: &Refused,
) -> Result<(), CmdError> {
    let text = format!(
        "{doing} still refused at {}, every gate run to its end:\n{}",
        refused.revision,
        refused.refusals.join("\n")
    );
    for defect in defects {
        process(OKO, &["defects", "note", defect, "--text", &text], None).await?;
    }
    Ok(())
}

/// Record on `defects` that `repository`@`revision` repaired them, as
/// `passed` says ("The delivery routine installed P:S from").
pub async fn repaired(
    defects: &[String],
    passed: &str,
    repository: &str,
    revision: &str,
) -> Result<(), CmdError> {
    let revision = format!("{repository}@{revision}");
    let detail = format!("{passed} {revision}");
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
