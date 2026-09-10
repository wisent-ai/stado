//! Queue every declared delivery of one published run, then collect the
//! verdict each one reached.

use std::collections::BTreeMap;

use serde_json::Map;

use crate::cli::release_submit::builds::builder::{builder, target_consumer};
use crate::cli::release_submit::builds::jobs::terminal::{
    job_output_tail, read_terminal_job, terminal,
};
use crate::cli::release_submit::builds::jobs::{input, secret_refs};
use crate::cli::release_submit::deliver::{delivery_job_command, DeliveryRequest};
use crate::cli::release_submit::run::source::{queue_immutable, run_path, run_uri};
use crate::cli::release_submit::run::state::save;
use crate::cli::CmdError;
use crate::models::job_state;
use crate::queue::storage::JobStorage;
use crate::queue::submit::{stable_run_id, submit_batch, SubmitOptions};
use crate::release_control::{self, ReleaseArtifactRef};
use crate::release_pipeline::{DeliveryRun, DeliveryRunState, ReleasePipelineManifest, ReleaseRun};

pub(crate) async fn run_deliveries(
    run: &mut ReleaseRun,
    m: &ReleasePipelineManifest,
    artifacts: &BTreeMap<String, ReleaseArtifactRef>,
) -> Result<(), CmdError> {
    let store = JobStorage::new().await?;
    for d in &m.deliveries {
        // The queue, not a stale release summary, decides whether an attempt
        // finished. A previous coordinator may have stopped before recording
        // failures from later deliveries.
        let prior_failure = match run.deliveries.get(&d.name) {
            Some(current) if current.state != DeliveryRunState::Passed => {
                read_terminal_job(&store, &current.job_id)
                    .await?
                    .filter(|job| {
                        matches!(job.state.as_str(), job_state::FAILED | job_state::CANCELLED)
                    })
            }
            _ => None,
        };
        if !run.deliveries.contains_key(&d.name) || prior_failure.is_some() {
            let a = &artifacts[&d.platform];
            let request = DeliveryRequest {
                schema_version: 1,
                run_id: run.run_id.clone(),
                name: d.name.clone(),
                product: run.product.clone(),
                version: run.version.clone(),
                platform: d.platform.clone(),
                argv: d.argv.clone(),
                required: d.required,
                secret_env: d.secret_env.clone(),
                source_path: "source.tar.gz".into(),
                source_uri: run.source_uri.clone(),
                source_sha256: run.source_sha256.clone(),
                archive_path: "release.tar.gz".into(),
                archive_uri: a.archive_uri.clone(),
                archive_sha256: a.artifact_sha256.clone(),
                manifest_uri: a.manifest_uri.clone(),
                manifest_sha256: a.manifest_sha256.clone(),
            };
            let bytes = serde_json::to_vec(&request)?;
            let sha = release_control::sha256_bytes(&bytes);
            let uri = run_uri(
                &run.product,
                &run.run_id,
                &format!("deliveries/{}/request.json", d.name),
            );
            queue_immutable(
                &run_path(
                    &run.product,
                    &run.run_id,
                    &format!("deliveries/{}/request.json", d.name),
                ),
                &bytes,
            )
            .await?;
            let mut resolved = Map::new();
            resolved.insert("request".into(), input(&uri, "delivery-request.json", &sha));
            resolved.insert(
                "archive".into(),
                input(&a.archive_uri, "release.tar.gz", &a.artifact_sha256),
            );
            resolved.insert(
                "source".into(),
                input(
                    &run_uri(&run.product, &run.run_id, "inputs/source.tar.gz"),
                    "source.tar.gz",
                    &run.source_sha256,
                ),
            );
            // A delivery that names its target runs ON that target and
            // installs locally; only target-less deliveries fall back to any
            // live builder of the platform.
            let consumer = if d.target.is_empty() {
                builder(&m.platforms[&d.platform].runner_platform, None, None)
                    .await?
                    .1
            } else {
                target_consumer(&d.target).await?
            };
            // Match platform retries: each failed job anchors exactly one
            // replacement, including a resume interrupted before save(run).
            let submission_run_id = match &prior_failure {
                Some(job) => stable_run_id(
                    "release-delivery",
                    &format!("{}\0{}\0{}", run.run_id, d.name, job.job_id),
                ),
                None => stable_run_id("release-delivery", &format!("{}\0{}", run.run_id, d.name)),
            };
            let options = SubmitOptions {
                pinned_host: consumer,
                priority: crate::constants::RELEASE_JOB_PRIORITY,
                run_id: submission_run_id,
                output_uri: run_uri(
                    &run.product,
                    &run.run_id,
                    &format!("deliveries/{}/output", d.name),
                ),
                input_artifacts: resolved.clone(),
                resolved_input_artifacts: resolved,
                secret_env: secret_refs(&d.secret_env),
                ..Default::default()
            };
            let command = delivery_job_command(&run.product).to_string();
            let mut jobs = submit_batch(std::slice::from_ref(&command), &options).await?;
            let job = jobs
                .pop()
                .ok_or_else(|| CmdError::click("durable delivery submission returned no job"))?;
            run.deliveries.insert(
                d.name.clone(),
                DeliveryRun {
                    name: d.name.clone(),
                    platform: d.platform.clone(),
                    job_id: job.job_id.clone(),
                    output_prefix: format!("status/{}/output/", job.job_id),
                    required: d.required,
                    state: DeliveryRunState::Submitted,
                    receipt_sha256: None,
                    failure: None,
                },
            );
            save(run).await?;
        }
    }

    // Queue every target before waiting for any one of them. A silent host
    // must not prevent later targets from receiving the same immutable
    // release: they are independent deliveries, even though their required
    // verdicts are collected into one release result.
    for d in &m.deliveries {
        let current = run.deliveries[&d.name].clone();
        if current.state == DeliveryRunState::Passed {
            continue;
        }
        let job = terminal(&store, &current.job_id).await?;
        let ok = matches!(
            job.state.as_str(),
            job_state::COMPLETED | job_state::UPLOADED
        );
        let receipt = store
            .read_bytes(&format!(
                "status/{}/output/delivery-receipt.json",
                current.job_id
            ))
            .await?;
        let updated = run.deliveries.get_mut(&d.name).unwrap();
        updated.receipt_sha256 = receipt.as_deref().map(release_control::sha256_bytes);
        updated.state = if ok {
            DeliveryRunState::Passed
        } else {
            DeliveryRunState::Failed
        };
        updated.failure = if ok {
            None
        } else {
            Some(format!(
                "{}{}",
                job.error.clone().unwrap_or_else(|| job.state.clone()),
                job_output_tail(&store, &current.job_id).await
            ))
        };
        if d.required && !ok {
            return Err(CmdError::click(format!(
                "required delivery {} failed",
                d.name
            )));
        }
    }
    Ok(())
}
