//! Turn one platform recipe into a durable, immutably requested build job on
//! an eligible fleet builder.

use std::collections::BTreeMap;

use serde_json::{Map, Value};

use crate::cli::release_submit::builds::builder::builder;
use crate::cli::release_submit::builds::jobs::command::release_worker_command;
use crate::cli::release_submit::builds::jobs::{input, persist_worker_request, secret_refs};
use crate::cli::release_submit::builds::scratch::last_scratch;
use crate::cli::release_submit::run::source::{queue_immutable, run_path, run_uri};
use crate::cli::storage;
use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::queue::submit::{stable_run_id, submit_batch, SubmitOptions};
use crate::release_control;
use crate::release_pipeline::{
    PlatformRun, PlatformRunState, ReleasePipelineManifest, WorkerInput, WorkerRequest,
};

// The build request's identity: every argument is a distinct coordinate the
// worker is required to receive, and each is already validated by the caller.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn enqueue(
    store: &JobStorage,
    id: &str,
    m: &ReleasePipelineManifest,
    version: &str,
    platform: &str,
    commit: &str,
    source_sha: &str,
    source_uri: &str,
    manifest_sha: &str,
    manifest_uri: &str,
    prior_terminal_job_id: Option<&str>,
) -> Result<PlatformRun, CmdError> {
    let submission_run_id = match prior_terminal_job_id {
        Some(prior_job_id) => stable_run_id(
            "release-platform",
            &format!("{id}\0{platform}\0{prior_job_id}"),
        ),
        None => stable_run_id("release-platform", &format!("{id}\0{platform}")),
    };
    let request_path = run_path(&m.product, id, &format!("requests/{platform}.json"));
    let uri = run_uri(&m.product, id, &format!("requests/{platform}.json"));
    let saved_bytes = store.read_bytes(&request_path).await?;
    let saved_request: Option<WorkerRequest> = saved_bytes
        .as_deref()
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|error| {
            CmdError::click(format!(
                "invalid saved release request {request_path}: {error}"
            ))
        })?;
    if saved_request
        .as_ref()
        .is_some_and(|request| request.builder.is_empty())
    {
        return Err(CmdError::click(format!(
            "saved release request {request_path} has no builder"
        )));
    }
    let saved_submission = if saved_request.is_some() {
        crate::queue::runs::read_run(store, &submission_run_id).await?
    } else {
        None
    };
    if saved_submission.is_none() {
        let queue_control = crate::queue::control::read(store)
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        if queue_control.paused {
            return Err(CmdError::click(format!(
                "release submission cannot enqueue {platform} while the queue is paused ({})",
                queue_control.pause_summary()
            )));
        }
    }
    let recipe = &m.platforms[platform];
    let scratch = last_scratch(store, &m.product, &recipe.runner_platform).await?;
    let (builder_name, consumer) = if let (Some(request), Some(submission)) =
        (&saved_request, &saved_submission)
    {
        let consumer = submission
            .get("request")
            .and_then(|request| request.get("options"))
            .and_then(|options| options.get("pinned_host"))
            .and_then(Value::as_str)
            .filter(|consumer| !consumer.is_empty())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "saved release submission {submission_run_id} has no pinned consumer"
                ))
            })?;
        (request.builder.clone(), consumer.to_owned())
    } else {
        let pinned = saved_request
            .as_ref()
            .map(|request| request.builder.as_str());
        let (host, consumer) = builder(&recipe.runner_platform, pinned, scratch.as_ref()).await?;
        (host.name, consumer)
    };
    let mut resolved = Map::new();
    resolved.insert(
        "source".into(),
        input(source_uri, "source.tar.gz", source_sha),
    );
    resolved.insert(
        "manifest".into(),
        input(manifest_uri, "release-manifest.json", manifest_sha),
    );
    let mut inputs = BTreeMap::new();
    for (name, v) in &m.inputs {
        let path = format!("input-archives/{name}.tar.gz");
        // Stage every declared input inside the queue namespace, which is where
        // the worker resolves objects, exactly as the source archive, manifest
        // and request are already staged.
        //
        // A cross-namespace pin such as `stado://sources/skarbiec/<sha>/...`
        // cannot be read by the worker at all: `StadoObjectBackend` builds
        // `ObjectRef::new(&self.namespace, path)`, so every read is re-prefixed
        // with the queue namespace, while `materialize_stado_inputs` hands it
        // `ecosystem/sources/...`. Publisher and worker computed different keys
        // for one object, and the build failed with `input input-skarbiec is
        // absent` naming an object that was on the store's disk the whole time.
        let leaf = format!("inputs/{name}.tar.gz");
        let staged_path = run_path(&m.product, id, &leaf);
        let staged_uri = run_uri(&m.product, id, &leaf);
        if saved_request.is_none() {
            let bytes = storage::fetch_object(&v.uri).await?;
            let staged_sha = release_control::sha256_bytes(&bytes);
            if staged_sha != v.sha256 {
                return Err(CmdError::click(format!(
                    "input {name} at {} hashes to {staged_sha}, recipe declares {}",
                    v.uri, v.sha256
                )));
            }
            queue_immutable(&staged_path, &bytes).await?;
        }
        resolved.insert(
            format!("input-{name}"),
            input(&staged_uri, &path, &v.sha256),
        );
        inputs.insert(
            name.clone(),
            WorkerInput {
                uri: staged_uri,
                sha256: v.sha256.clone(),
                archive_path: path,
                mount: v.mount.clone(),
                extract: v.extract,
            },
        );
    }
    let request = WorkerRequest {
        schema_version: 1,
        run_id: id.into(),
        product: m.product.clone(),
        version: version.into(),
        platform: platform.into(),
        builder: builder_name.clone(),
        source_commit: commit.into(),
        source_sha256: source_sha.into(),
        manifest_sha256: manifest_sha.into(),
        source_archive: "source.tar.gz".into(),
        manifest_path: "release-manifest.json".into(),
        inputs,
        secret_env: recipe.secret_env.clone(),
    };
    let (request, bytes) = persist_worker_request(
        store,
        &request_path,
        request,
        saved_request.zip(saved_bytes),
    )
    .await?;
    let consumer = if request.builder == builder_name {
        consumer
    } else {
        // Another coordinator published the request first. Keep that placement
        // and apply the normal claim gate before creating its queue plan.
        builder(
            &recipe.runner_platform,
            Some(&request.builder),
            scratch.as_ref(),
        )
        .await?
        .1
    };
    let sha = release_control::sha256_bytes(&bytes);
    resolved.insert("request".into(), input(&uri, "release-request.json", &sha));
    let output_uri = match prior_terminal_job_id {
        Some(_) => run_uri(
            &m.product,
            id,
            &format!("platforms/{platform}/attempts/{submission_run_id}/output"),
        ),
        None => run_uri(&m.product, id, &format!("platforms/{platform}/output")),
    };
    let command = release_worker_command(&output_uri);
    let options = SubmitOptions {
        pinned_host: consumer,
        priority: crate::primitives::constants::RELEASE_JOB_PRIORITY,
        run_id: submission_run_id,
        output_uri,
        input_artifacts: resolved.clone(),
        resolved_input_artifacts: resolved,
        secret_env: secret_refs(&recipe.secret_env),
        ..Default::default()
    };
    let mut jobs = submit_batch(std::slice::from_ref(&command), &options).await?;
    let job = jobs
        .pop()
        .ok_or_else(|| CmdError::click("durable release submission returned no job"))?;
    Ok(PlatformRun {
        platform: platform.into(),
        builder: request.builder,
        job_id: job.job_id.clone(),
        output_prefix: format!("status/{}/output/", job.job_id),
        state: PlatformRunState::Submitted,
        artifact_sha256: None,
        release_manifest_sha256: None,
        qualification_uri: None,
        failure: None,
    })
}
