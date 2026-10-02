//! Queue one declared delivery of a published run: write its immutable
//! request, pick where it runs and submit its job, unless an earlier pass
//! already queued it and that job has not failed.

use std::collections::BTreeMap;

use serde_json::Map;

use crate::cli::release_submit::builds::builder::{builder, target_consumer};
use crate::cli::release_submit::builds::jobs::terminal::read_terminal_job;
use crate::cli::release_submit::builds::jobs::{input, secret_refs};
use crate::cli::release_submit::deliver::{delivery_job_command, DeliveryRequest};
use crate::cli::release_submit::run::source::{
    queue_immutable, run_path, run_source_input_uri, run_uri,
};
use crate::cli::release_submit::run::state::save;
use crate::cli::CmdError;
use crate::models::job_state;
use crate::queue::storage::JobStorage;
use crate::queue::submit::{stable_run_id, submit_batch, SubmitOptions};
use crate::release_control::{self, ReleaseArtifactRef};
use crate::release_pipeline::{
    Delivery, DeliveryRun, DeliveryRunState, ReleasePipelineManifest, ReleaseRun,
};

/// A delivery that could not be queued, recorded failed with `failure` and
/// no job, so the next pass places it again.
pub(super) fn record_unqueued(run: &mut ReleaseRun, d: &Delivery, failure: String) {
    run.deliveries.insert(
        d.name.clone(),
        DeliveryRun {
            name: d.name.clone(),
            platform: d.platform.clone(),
            job_id: String::new(),
            output_prefix: String::new(),
            required: d.required,
            state: DeliveryRunState::Failed,
            receipt_sha256: None,
            failure: Some(failure),
        },
    );
}

pub(super) async fn queue_delivery(
    run: &mut ReleaseRun,
    m: &ReleasePipelineManifest,
    artifacts: &BTreeMap<String, ReleaseArtifactRef>,
    store: &JobStorage,
    d: &Delivery,
) -> Result<(), CmdError> {
    // The queue, not a stale release summary, decides whether an attempt
    // finished. A previous coordinator may have stopped before recording
    // failures from later deliveries.
    let prior_failure = match run.deliveries.get(&d.name) {
        Some(current)
            if current.state != DeliveryRunState::Passed && !current.job_id.is_empty() =>
        {
            read_terminal_job(store, &current.job_id)
                .await?
                .filter(|job| {
                    matches!(job.state.as_str(), job_state::FAILED | job_state::CANCELLED)
                })
        }
        _ => None,
    };
    // A delivery a previous pass could not place has no job to wait for;
    // it is placed again, now that its target may publish capacity.
    let unplaced = run
        .deliveries
        .get(&d.name)
        .is_some_and(|current| current.job_id.is_empty());
    if run.deliveries.contains_key(&d.name) && prior_failure.is_none() && !unplaced {
        return Ok(());
    }
    let Some(a) = artifacts.get(&d.platform) else {
        record_unqueued(run, d, format!("no published artifact for delivery platform {}", d.platform));
        save(run).await?;
        return Ok(());
    };
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
    let request_key = format!("deliveries/{}/request.json", d.name);
    let uri = run_uri(&run.product, &run.run_id, &request_key);
    queue_immutable(&run_path(&run.product, &run.run_id, &request_key), &bytes).await?;
    let mut resolved = Map::new();
    resolved.insert("request".into(), input(&uri, "delivery-request.json", &sha));
    resolved.insert(
        "archive".into(),
        input(&a.archive_uri, "release.tar.gz", &a.artifact_sha256),
    );
    resolved.insert(
        "source".into(),
        input(
            &run_source_input_uri(run),
            "source.tar.gz",
            &run.source_sha256,
        ),
    );
    let target = d.target.host().map_err(CmdError::click)?;
    let consumer = if target.is_empty() {
        builder(
            &crate::cli::release_submit::builds::builder::Fleet::read().await?,
            &m.platforms[&d.platform].runner_platform,
            None,
            None,
            &d.secret_env,
            &BTreeMap::new(),
        )
        .await?
        .1
    } else {
        // One target that publishes no capacity would fail the whole run
        // here, before any delivery was queued, so a release published while
        // one host is out of disk would reach no host at all. That target's
        // delivery is recorded failed with the refusal and the rest are
        // queued.
        match target_consumer(target).await {
            Ok(consumer) => consumer,
            Err(refusal) => {
                record_unqueued(run, d, format!("not queued on {target}: {refusal}"));
                save(run).await?;
                return Ok(());
            }
        }
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
        priority: crate::primitives::constants::RELEASE_JOB_PRIORITY,
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
    Ok(())
}
