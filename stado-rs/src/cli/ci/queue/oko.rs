//! What the delivery queue writes in Oko's defect register, through Oko's
//! own CLI: one defect per refused candidate with every refusal of its run,
//! owned by the session whose recorded repair rides on it, and the repair
//! recorded on that defect once a later candidate of the same checkout is
//! admitted to `main`.

use std::path::Path;

use serde::Deserialize;

use super::{register::Entry, MAIN};
use crate::cli::release_loop::{open_repairs, process::run, process::succeeds, repository_of};
use crate::cli::CmdError;

/// Oko's program; the queue records in Oko through its CLI only.
const OKO: &str = "oko";

/// The sessions owning the open defects whose recorded repair lies between
/// `main` (excluded) and `candidate` (included) in `checkout`: the authors
/// of the work the candidate carries onto `main`, each named once.
pub async fn authors(
    checkout: &Path,
    main: &str,
    candidate: &str,
) -> Result<Vec<String>, CmdError> {
    let repository = repository_of(checkout)?;
    let mut authors: Vec<String> = Vec::new();
    for repair in open_repairs()
        .await?
        .into_iter()
        .filter(|repair| repair.repository == repository)
    {
        let Some(owner) = repair.owner else {
            continue;
        };
        if authors.contains(&owner) {
            continue;
        }
        let carried = succeeds(
            "git",
            &["merge-base", "--is-ancestor", &repair.commit, candidate],
            checkout,
        )
        .await?;
        let landed = succeeds(
            "git",
            &["merge-base", "--is-ancestor", &repair.commit, main],
            checkout,
        )
        .await?;
        if carried && !landed {
            authors.push(owner);
        }
    }
    Ok(authors)
}

#[derive(Deserialize)]
struct Recorded {
    id: String,
}

/// Record the refused `entry` as one defect carrying every refusal of its
/// run, owned by its first author with the others noted on it; its id.
pub async fn record_refused(entry: &Entry) -> Result<String, CmdError> {
    let repository = repository_of(&entry.checkout)?;
    let title = format!(
        "{} candidate {} was refused by the delivery queue with {} installation refusals, so main stays at {}",
        entry.product,
        entry.candidate,
        entry.refusals.len(),
        entry.main
    );
    let surface = format!(
        "{} delivery branch at {} (stado ci deliver --checkout {})",
        entry.product,
        entry.candidate,
        entry.checkout.display()
    );
    let evidence = format!(
        "The delivery queue's builder ran every installation gate over {} to its end; every refusal:\n{}",
        entry.candidate,
        entry.refusals.join("\n")
    );
    let impact = format!(
        "{MAIN} stays at {} until a candidate that passes every installation gate is pushed to the delivery branch",
        entry.main
    );
    let found = format!("{repository}@{}", entry.candidate);
    let recorded = run(
        OKO,
        &[
            "defects",
            "record",
            "--product",
            &entry.product,
            "--title",
            &title,
            "--surface",
            &surface,
            "--evidence",
            &evidence,
            "--impact",
            &impact,
            "--repair",
            "Repair every refusal named in the evidence in the source it names, commit, and push the delivery branch again; the queue gates the new tip and moves main when it passes",
            "--found-in",
            &found,
            "--json",
        ],
        None,
    )
    .await?;
    let id = serde_json::from_str::<Recorded>(&recorded)
        .map_err(|error| {
            CmdError::click(format!(
                "oko defects record answered no recorded defect: {error}: {recorded}"
            ))
        })?
        .id;
    if let Some((owner, others)) = entry.authors.split_first() {
        run(
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
        if !others.is_empty() {
            let text = format!(
                "The refused candidate also carries recorded repairs of these sessions: {}",
                others.join(", ")
            );
            run(OKO, &["defects", "note", &id, "--text", &text], None).await?;
        }
    }
    Ok(id)
}

/// Record on `defect` that `admitted` of `checkout`, which passed every
/// installation gate and now stands on `main`, repairs it.
pub async fn record_repaired(
    defect: &str,
    checkout: &Path,
    admitted: &str,
) -> Result<(), CmdError> {
    let revision = format!("{}@{admitted}", repository_of(checkout)?);
    let detail = format!(
        "The delivery queue admitted {admitted} to main after every installation gate passed over it to its end"
    );
    run(
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
    .await
    .map(drop)
}
