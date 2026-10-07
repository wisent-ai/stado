use super::{Change, ChangeStatus};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{
    BuildReceipt, BuildRun, BuildRunState, PlatformRunState, ProductManifest, StepStatus,
};
use futures::StreamExt;
use std::collections::HashMap;

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
        .list_paths("runs/build/", 0)
        .await
        .map_err(CmdError::from)?
        .into_iter()
        .filter(|path| is_batch(path))
        .collect();
    let answers = futures::stream::iter(&batches)
        .map(|path| batch_observation(store, path, wanted))
        .buffered(crate::queue::copy::default_concurrency())
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
    let name = path.rsplit('/').next().unwrap_or(path);
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
    let root = &path[..path.rfind('/').map_or(0, |slash| slash + 1)];
    // A terminal build's observation never changes: its receipts are
    // immutable and nothing re-enters a finished build. It is kept beside the
    // batch the first time it is read, and later reads take that one file
    // instead of the run, the manifest and every platform's receipt, which
    // over a hundred builds cost `changes list` 78 s.
    let kept_path = format!("{root}observation.json");
    if let Some(text) = store
        .download_text(&kept_path)
        .await
        .map_err(CmdError::from)?
    {
        if let Ok(observation) = serde_json::from_str::<Observation>(&text) {
            return Ok(Some((batch, observation)));
        }
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
        let _ = store
            .create_text_if_absent(&kept_path, &serde_json::to_string(&observation)?)
            .await;
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
            run_id: Some(value.run_id.clone()),
            failure: value.failure.clone(),
            evidence: value.evidence.clone(),
        },
        None => ChangeStatus {
            change,
            state: "queued".into(),
            run_id: None,
            failure: None,
            evidence: Vec::new(),
        },
    }
}

async fn observe(store: &JobStorage, run: &mut BuildRun) -> Result<Observation, CmdError> {
    let manifest_path =
        crate::cli::release_submit::build_path(&run.product, &run.build_id, "manifest.json");
    let manifest_bytes = store.read_bytes(&manifest_path).await?.ok_or_else(|| {
        CmdError::click(format!("qualification manifest missing: {manifest_path}"))
            .stating(crate::primitives::failure::FailureCode::NotFound)
    })?;
    if crate::release_control::sha256_bytes(&manifest_bytes) != run.manifest_sha256 {
        return Err(CmdError::click("qualification manifest digest mismatch")
            .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let ProductManifest::Release(manifest) =
        crate::release_pipeline::parse_product_manifest(&manifest_bytes)
            .map_err(CmdError::declaration)?
    else {
        return Err(CmdError::refused(
            "qualification manifest declares no releases",
        ));
    };
    // Read terminal jobs without queueing a retry or requiring a release.
    crate::cli::release_submit::refresh_build(store, run, &manifest).await?;
    let mut result = Observation {
        created_at: run.created_at.clone(),
        state: "building".into(),
        run_id: run.build_id.clone(),
        failure: None,
        evidence: Vec::new(),
    };
    let mut qualified = manifest
        .platforms
        .iter()
        .filter(|(_, recipe)| recipe.required)
        .all(|(platform, _)| run.platforms.contains_key(platform))
        && !run.platforms.is_empty();
    for (platform, entry) in &run.platforms {
        let recipe = manifest.platforms.get(platform).ok_or_else(|| {
            CmdError::click(format!(
                "qualification platform absent from manifest: {platform}"
            ))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
        })?;
        let path = format!("status/{}/output/receipt.json", entry.job_id);
        let Some(bytes) = store.read_bytes(&path).await? else {
            if recipe.required {
                qualified = false;
            }
            continue;
        };
        let receipt: BuildReceipt = serde_json::from_slice(&bytes)?;
        if receipt.run_id != run.build_id
            || receipt.job_id != entry.job_id
            || receipt.platform != *platform
            || receipt.product != run.product
            || receipt.version != run.version
            || receipt.builder != entry.builder
            || receipt.source_commit != run.source_commit
            || receipt.source_sha256 != run.source_sha256
            || receipt.manifest_sha256 != run.manifest_sha256
        {
            return Err(
                CmdError::click(format!("qualification identity mismatch at {path}"))
                    .stating(crate::primitives::failure::FailureCode::InfraDown),
            );
        }
        if !recipe.required {
            result.evidence.push(receipt);
            continue;
        }
        // A platform qualifies on the tests it declares; one that declares
        // none (no test the operator approved) qualifies on its build alone.
        let complete = recipe.tests.iter().all(|test| {
            let name = format!("test:{}", test.name);
            let mut recorded = receipt.quality.iter().filter(|step| step.name == name);
            recorded.next().is_some_and(|step| {
                step.argv == test.argv
                    && step.status == StepStatus::Passed
                    && step.exit_code == Some(0)
            }) && recorded.next().is_none()
        });
        if receipt.status == StepStatus::Failed {
            result.state = "failed".into();
            result.failure = Some(format!(
                "{platform}: {}",
                receipt
                    .failure
                    .as_deref()
                    .unwrap_or("worker failed without a cause")
            ));
        } else if receipt.build.status != StepStatus::Passed
            || receipt.build.exit_code != Some(0)
            || !complete
        {
            qualified = false;
            if result.state != "failed" {
                result.state = "awaiting_tests".into();
                result.failure = Some(format!(
                    "{platform}: no complete passing post-build test evidence"
                ));
            }
        }
        result.evidence.push(receipt);
    }
    if run.state == BuildRunState::Failed {
        result.state = "failed".into();
        if result.failure.is_none() {
            result.failure = run.failure.clone().or_else(|| {
                let reasons: Vec<_> = run
                    .platforms
                    .iter()
                    .filter_map(|(name, platform)| {
                        platform
                            .failure
                            .as_ref()
                            .map(|reason| format!("{name}: {reason}"))
                    })
                    .collect();
                Some(if reasons.is_empty() {
                    "build failed without a recorded cause".into()
                } else {
                    reasons.join("; ")
                })
            });
        }
    } else if qualified && run.state == BuildRunState::Passed && result.state != "failed" {
        result.state = "passed".into();
    }
    Ok(result)
}
