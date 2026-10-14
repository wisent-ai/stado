//! What one build observed for the changes its batch covers: its run read
//! against its jobs and every platform's receipt, judged by the manifest the
//! build was made from.

use super::{evidence, Observation};
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::{BuildReceipt, BuildRun, BuildRunState, ProductManifest, StepStatus};

/// The observation of `run`. A build made from a manifest this Stado's
/// release contract no longer reads can never be read against its jobs or
/// qualify: the changes it covers are observed failed, naming the manifest
/// and the contract error, instead of failing every reader of the batch
/// history.
pub(super) async fn observe(
    store: &JobStorage,
    run: &mut BuildRun,
) -> Result<Observation, CmdError> {
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
    let manifest = match crate::release_pipeline::parse_product_manifest(&manifest_bytes) {
        Ok(ProductManifest::Release(manifest)) => manifest,
        Ok(ProductManifest::NonRelease(_)) => {
            return Err(CmdError::refused(
                "qualification manifest declares no releases",
            ))
        }
        Err(error) => {
            return Ok(Observation {
                created_at: run.created_at.clone(),
                state: "failed".into(),
                run_id: run.build_id.clone(),
                failure: Some(format!(
                    "{manifest_path} no longer reads under this Stado's release contract: {error}"
                )),
                evidence: Vec::new(),
            })
        }
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
        if receipt.status == StepStatus::Failed {
            result.state = "failed".into();
            result.failure = Some(match receipt.failure.as_deref() {
                Some(cause) => format!("{platform}: {cause}"),
                None => {
                    format!("{platform}: the worker's receipt says failed and records no cause")
                }
            });
        } else if let Some(reason) = evidence::incomplete(recipe, &receipt) {
            qualified = false;
            if result.state != "failed" {
                result.state = "awaiting_tests".into();
                result.failure = Some(format!("{platform}: {reason}"));
            }
        }
        result.evidence.push(receipt);
    }
    if run.state == BuildRunState::Failed {
        result.state = "failed".into();
        if result.failure.is_none() {
            result.failure = Some(match &run.failure {
                Some(cause) => cause.clone(),
                None => run_failure_reasons(run),
            });
        }
    } else if qualified && run.state == BuildRunState::Passed && result.state != "failed" {
        result.state = "passed".into();
    }
    Ok(result)
}

/// Why a failed run failed, read from its platforms when the run records no
/// cause of its own.
fn run_failure_reasons(run: &BuildRun) -> String {
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
    match reasons.is_empty() {
        true => "build failed without a recorded cause".into(),
        false => reasons.join("; "),
    }
}
