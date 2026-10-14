use super::{Change, ChangeStatus};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{BuildReceipt, BuildRun, BuildRunState, PlatformRunState};
use futures::StreamExt;
use std::collections::HashMap;

mod evidence;
mod observe;

use observe::observe;

pub(super) const QUALIFICATION_POLICY: &str = "required-behavioral-tests-v1";

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(super) struct Observation {
    created_at: String,
    run_id: String,
    state: String,
    failure: Option<String>,
    evidence: Vec<BuildReceipt>,
}

/// Read each build and receipt once, even when it covers many changes.
///
/// Independent frozen batches share the bulk object-read concurrency budget.
/// Each batch already contains its frozen tickets, so `frozen` indexes those
/// by ID rather than downloading the same ticket again for every change.
pub(super) async fn observations(
    store: &JobStorage,
    wanted: &std::collections::HashSet<String>,
) -> Result<Observed, CmdError> {
    let batches: Vec<String> = store
        .list_blobs_with_meta("runs/build/")
        .await
        .map_err(CmdError::from)?
        .into_iter()
        .map(|blob| blob.name)
        .filter(|path| is_batch(path))
        .collect();
    let answers = futures::stream::iter(&batches)
        .map(|path| batch_observation(store, path, wanted))
        .buffered(crate::queue::copy::default_concurrency()?)
        .collect::<Vec<_>>()
        .await;
    let mut observed = Observed::default();
    for answer in answers {
        let Some((batch, observation)) = answer? else {
            continue;
        };
        for change in batch {
            if observed.by_change.get(&change.id).is_none_or(|old| {
                (&observation.created_at, &observation.run_id) > (&old.created_at, &old.run_id)
            }) {
                observed
                    .by_change
                    .insert(change.id.clone(), observation.clone());
            }
            observed.frozen.insert(change.id.clone(), change);
        }
    }
    Ok(observed)
}

/// A build's batch file: its first freeze `changes.json`, or an additional
/// `changes-<digest>.json` bound later to the same build.
fn is_batch(path: &str) -> bool {
    let name = match path.rsplit_once('/') {
        Some((_, name)) => name,
        None => path,
    };
    name == "changes.json" || (name.starts_with("changes-") && name.ends_with(".json"))
}

/// What the build history says about the wanted tickets.
#[derive(Default)]
pub(super) struct Observed {
    /// The newest build observation covering each ticket.
    pub(super) by_change: HashMap<String, Observation>,
    /// Each covered ticket as its batch froze it.
    pub(super) frozen: HashMap<String, Change>,
}

/// One frozen batch and what its build observed; nothing for an empty batch,
/// a batch that covers none of the `wanted` changes, or a build whose run
/// record is gone. Observing a batch reads its run, its manifest and every
/// job of the build; doing that for the whole build history made `changes
/// list` take 124 s when it answers only for the changes still listed.
async fn batch_observation(
    store: &JobStorage,
    path: &str,
    wanted: &std::collections::HashSet<String>,
) -> Result<Option<(Vec<Change>, Observation)>, CmdError> {
    let text = store
        .download_text(path)
        .await
        .map_err(CmdError::from)?
        .ok_or_else(|| {
            CmdError::click(format!("build batch missing: {path}"))
                .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    let batch: Vec<Change> = serde_json::from_str(&text)?;
    if batch.is_empty() || !batch.iter().any(|change| wanted.contains(&change.id)) {
        return Ok(None);
    }
    let root = match path.rsplit_once('/') {
        Some((directory, _)) => format!("{directory}/"),
        None => String::new(),
    };
    // A terminal build's observation never changes: its receipts are
    // immutable and nothing re-enters a finished build. It is kept beside the
    // batch the first time it is read, and later reads take that one file
    // instead of the run, the manifest and every platform's receipt, which
    // over a hundred builds cost `changes list` 78 s.
    let kept_path = format!("{root}{QUALIFICATION_POLICY}.json");
    if let Some(text) = store
        .download_text(&kept_path)
        .await
        .map_err(CmdError::from)?
    {
        let observation = serde_json::from_str::<Observation>(&text).map_err(|error| {
            CmdError::click(format!(
                "cannot decode qualification observation {kept_path}: {error}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        return Ok(Some((batch, observation)));
    }
    let run_path = format!("{root}run.json");
    let Some(text) = store
        .download_text(&run_path)
        .await
        .map_err(CmdError::from)?
    else {
        return Ok(None);
    };
    let mut run: BuildRun = serde_json::from_str(&text)?;
    let observation = observe(store, &mut run).await?;
    if batch.iter().any(|change| change.product != run.product) {
        return Err(CmdError::click("build batch product mismatch")
            .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    // Settled means nothing it is built from can change: the run is terminal
    // and no platform, optional ones included, is still building.
    let settled = matches!(run.state, BuildRunState::Passed | BuildRunState::Failed)
        && run
            .platforms
            .values()
            .all(|platform| platform.state != PlatformRunState::Submitted);
    if settled {
        // A settled observation that is not kept is recomputed from a run
        // that may since be pruned, so a failed write is the caller's error.
        store
            .create_text_if_absent(&kept_path, &serde_json::to_string(&observation)?)
            .await
            .map_err(CmdError::from)?;
    }
    Ok(Some((batch, observation)))
}

pub(super) fn for_change(
    change: Change,
    observations: &HashMap<String, Observation>,
) -> ChangeStatus {
    match observations.get(&change.id) {
        Some(value) => ChangeStatus {
            change,
            state: value.state.clone(),
            qualification_policy: QUALIFICATION_POLICY,
            run_id: Some(value.run_id.clone()),
            failure: value.failure.clone(),
            evidence: value.evidence.clone(),
        },
        None => ChangeStatus {
            change,
            state: "queued".into(),
            qualification_policy: QUALIFICATION_POLICY,
            run_id: None,
            failure: None,
            evidence: Vec::new(),
        },
    }
}
