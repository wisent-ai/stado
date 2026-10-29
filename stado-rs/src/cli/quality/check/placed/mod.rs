//! `stado quality check --place`: the check of one committed revision run as
//! placed work on the fleet host with room for it, instead of on the
//! machine that asked.
//!
//! The check is a queue job pinned to the host the placement chose
//! ([`crate::placement::hosts`]), marked exclusive: its host claims it only
//! while it runs nothing else and claims nothing else beside it, so a host
//! takes one check at a time and the rest queue. A product has one check in
//! flight: a newer revision's check cancels the older one, queued or
//! compiling, once its own job is queued; a check of the revision already in
//! flight follows that job instead of queueing a second. The revision
//! travels as a Git bundle in the fleet store ([`bundle`]), and the host
//! runs `stado quality check --bundle` over it, the same check the checkout
//! would run here.

mod bundle;
mod flight;
mod follow;
mod host;

pub(crate) use host::check_bundle;

use std::path::Path;

use serde_json::json;

use crate::cli::CmdError;
use crate::placement::hosts::{self, Standing};
use crate::queue::submit::{
    submit_batch, ResolvedHardwareProjection, SubmitOptions, CPU_MACHINE_TYPE,
};

/// The placed work this is, as hosts publish what they run.
const WORK: &str = "quality-check";

/// The queue batch every placed check carries, so the checks in flight are
/// read from the queue itself.
const BATCH: &str = "quality-check";

/// The product `revision` of `checkout` declares in its release manifest.
fn product_of(checkout: &Path, revision: &str) -> Result<String, CmdError> {
    let shown = format!(
        "git -C {} show {revision}:.wisent-release.json",
        checkout.display()
    );
    let output = crate::wait::output(
        std::process::Command::new("git")
            .arg("-C")
            .arg(checkout)
            .args(["show", &format!("{revision}:.wisent-release.json")]),
    )
    .map_err(|cause| CmdError::click(format!("`{shown}` did not run: {cause}")))?;
    if !output.status.success() {
        return Err(CmdError::refused(format!(
            "{revision} of {} holds no readable .wisent-release.json, so it declares no product \
             whose gates a placed check could run: {}",
            checkout.display(),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    let manifest: serde_json::Value = serde_json::from_slice(&output.stdout)
        .map_err(|cause| CmdError::refused(format!("`{shown}` is not JSON: {cause}")))?;
    manifest["product"]
        .as_str()
        .filter(|product| !product.is_empty())
        .map(str::to_string)
        .ok_or_else(|| {
            CmdError::refused(format!(
                "the .wisent-release.json of {revision} names no product"
            ))
        })
}

/// Place the check of `revision` (else the revision an install builds) of
/// the checkout at `root` (else the working directory), on `requested` when
/// named, and follow it to its end; exit 0 only when the placed check passed.
pub(crate) async fn place(
    root: Option<&str>,
    revision: Option<&str>,
    requested: Option<&str>,
    purposes: &[String],
    json: bool,
) -> Result<(), CmdError> {
    let checkout = match root {
        Some(path) => std::path::absolute(path),
        None => std::env::current_dir(),
    }
    .map_err(|error| {
        CmdError::click(format!("cannot resolve the checkout to check: {error}"))
            .stating(crate::cli::entry::error::io_failure_code(error.kind()))
    })?;
    let revision = match revision {
        Some(revision) => super::command::resolve(&checkout, revision)?,
        None => super::command::built_revision(&checkout)?,
    };
    let purposes: Vec<&str> = purposes.iter().map(String::as_str).collect();
    placed(&checkout, &revision, requested, &purposes, json)
        .await
        .map_err(|error| error.machine_readable(json))
}

async fn placed(
    checkout: &Path,
    revision: &str,
    requested: Option<&str>,
    purposes: &[&str],
    json: bool,
) -> Result<(), CmdError> {
    let product = product_of(checkout, revision)?;
    let flight = flight::in_flight().await?;
    if let Some((job, _, _)) = flight
        .iter()
        .find(|(_, p, r)| *p == product && r == revision)
    {
        let state = if job.state == crate::queue::runs::QUEUE {
            "queued"
        } else {
            "running"
        };
        eprintln!(
            "stado quality check: {product} {revision} is already {state} as job {} on {}; following it",
            job.job_id, job.pinned_host
        );
        return finish(
            &product,
            revision,
            &job.job_id,
            &job.pinned_host,
            json!([]),
            &[],
            json,
        )
        .await;
    }
    let (registry, targets) = hosts::fleet_hosts(requested).await?;
    let verdicts = hosts::judge(&registry, targets, WORK).await?;
    let record = hosts::record(&verdicts);
    let Some(chosen) = verdicts
        .iter()
        .find(|verdict| !matches!(verdict.standing, Standing::Refused(_)))
    else {
        let said: Vec<String> = verdicts.iter().map(hosts::Verdict::describe).collect();
        let said = if said.is_empty() {
            "the registry declares no reachable host".to_string()
        } else {
            said.join("; ")
        };
        let cloud = hosts::credit_capacity(WORK).await;
        return Err(CmdError::refused(format!(
            "no fleet host can run the check of {product} {revision}: {said}; {cloud}"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    };
    for verdict in &verdicts {
        eprintln!("stado quality check: placement: {}", verdict.describe());
    }
    let target = chosen.target.name.clone();
    let store = crate::queue::submit::default_store("")
        .await
        .map_err(|error| {
            CmdError::from(error).within("open the fleet store for the source bundle")
        })?;
    let key = bundle::key(&product, revision);
    let area = bundle::area(revision)?;
    let file = area.join("source.bundle");
    let carried = match bundle::create(checkout, revision, &file) {
        Ok(()) => bundle::upload(&store, &key, &file).await,
        Err(error) => Err(error),
    };
    let removed = bundle::remove(&area);
    carried?;
    removed?;
    // The worker's own Stado runs the check (`STADO_BIN`, which the agent
    // sets for every job it starts): on a host that is the installed one.
    let mut command = format!(
        "exec \"$STADO_BIN\" quality check --bundle {} --revision {revision}",
        crate::deploy::shlex_quote(&key)
    );
    for purpose in purposes {
        command.push_str(&format!(
            " --purpose {}",
            crate::deploy::shlex_quote(purpose)
        ));
    }
    let stamp = chrono::Utc::now().format("%Y%m%dT%H%M%S%3f");
    let options = SubmitOptions {
        batch_id: BATCH.to_string(),
        run_id: format!(
            "{BATCH}-{product}-{revision}-{stamp}-{}",
            std::process::id()
        ),
        pinned_host: crate::cli::submit::resolve_pinned_host(&target).await?,
        exclusive: true,
        // The host runs nothing beside it, so it asks for the cores the host
        // published free; the agent hands them to Cargo (`CARGO_BUILD_JOBS`).
        cpu_cores: chosen.free_cores.unwrap_or_default().max(1),
        resolved_hardware: Some(ResolvedHardwareProjection {
            gpu_mem_gb: Default::default(),
            gpu_type: String::new(),
            machine_type: CPU_MACHINE_TYPE.to_string(),
        }),
        ..SubmitOptions::default()
    };
    let jobs = submit_batch(std::slice::from_ref(&command), &options)
        .await
        .map_err(|error| {
            let mut wrapped = CmdError::click(format!(
                "the check of {product} {revision} could not be queued on {target}: {error}"
            ));
            wrapped.failure = CmdError::from(error).failure;
            wrapped
        })?;
    let job = jobs.first().ok_or_else(|| {
        CmdError::click(format!(
            "the check of {product} {revision} was queued as no job"
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    eprintln!(
        "stado quality check: {product} {revision} queued on {target} as job {}",
        job.job_id
    );
    let superseded = flight::supersede(&store, &flight, &product, revision).await;
    finish(
        &product,
        revision,
        &job.job_id,
        &target,
        record,
        &superseded,
        json,
    )
    .await
}

/// Follow job `job_id` to its end and answer how the placed check ended.
async fn finish(
    product: &str,
    revision: &str,
    job_id: &str,
    target: &str,
    placement: serde_json::Value,
    superseded: &[String],
    json: bool,
) -> Result<(), CmdError> {
    let ending = follow::follow(job_id).await?;
    if let Some(log) = &ending.log {
        if json {
            eprint!("{log}")
        } else {
            print!("{log}")
        }
    }
    let passed = [crate::queue::runs::COMPLETED, crate::queue::runs::UPLOADED]
        .contains(&ending.state.as_str());
    if json {
        println!(
            "{}",
            crate::deploy::host_recovery::to_sorted_pretty(&json!({
                "status": if passed { "passed" } else { "failed" },
                "product": product,
                "revision": revision,
                "target": target,
                "job_id": job_id,
                "placement": placement,
                "superseded": superseded,
                "ending": follow::to_json(&ending),
            }))
        );
    }
    if passed {
        return Ok(());
    }
    Err(CmdError::refused(format!(
        "the placed check of {product} {revision} on {target} (job {job_id}) ended {}{}; its log is \
         above and in `stado job watch {job_id}`",
        ending.state,
        ending.error.map(|error| format!(": {error}")).unwrap_or_default()
    )))
}
