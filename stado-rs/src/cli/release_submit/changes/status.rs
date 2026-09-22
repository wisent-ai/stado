use super::{failure, Change, ChangeStatus};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{BuildReceipt, BuildRun, BuildRunState, ProductManifest, StepStatus};
use std::collections::HashMap;

#[derive(Clone)]
pub(super) struct Observation {
    created_at: String,
    run_id: String,
    state: String,
    failure: Option<String>,
    evidence: Vec<BuildReceipt>,
}

/// Read each build and receipt once, even when it covers many changes.
pub(super) async fn observations(
    store: &JobStorage,
) -> Result<HashMap<String, Observation>, CmdError> {
    let mut observations: HashMap<String, Observation> = HashMap::new();
    for path in store.list_paths("runs/build/", 0).await.map_err(failure)? {
        if !path.ends_with("/changes.json") {
            continue;
        }
        let text = store
            .download_text(&path)
            .await
            .map_err(failure)?
            .ok_or_else(|| CmdError::click(format!("build batch missing: {path}")))?;
        let batch: Vec<Change> = serde_json::from_str(&text)?;
        if batch.is_empty() {
            continue;
        }
        let run_path = format!("{}run.json", path.trim_end_matches("changes.json"));
        let Some(text) = store.download_text(&run_path).await.map_err(failure)? else {
            continue;
        };
        let mut run: BuildRun = serde_json::from_str(&text)?;
        let observation = observe(store, &mut run).await?;
        for change in batch {
            if run.product != change.product {
                return Err(CmdError::click("build batch product mismatch"));
            }
            if observations.get(&change.id).is_none_or(|old| {
                (&observation.created_at, &observation.run_id) > (&old.created_at, &old.run_id)
            }) {
                observations.insert(change.id, observation.clone());
            }
        }
    }
    Ok(observations)
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
    })?;
    if crate::release_control::sha256_bytes(&manifest_bytes) != run.manifest_sha256 {
        return Err(CmdError::click("qualification manifest digest mismatch"));
    }
    let ProductManifest::Release(manifest) =
        crate::release_pipeline::parse_product_manifest(&manifest_bytes).map_err(failure)?
    else {
        return Err(CmdError::click(
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
            return Err(CmdError::click(format!(
                "qualification identity mismatch at {path}"
            )));
        }
        if !recipe.required {
            result.evidence.push(receipt);
            continue;
        }
        let complete = !recipe.tests.is_empty()
            && recipe.tests.iter().all(|test| {
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
