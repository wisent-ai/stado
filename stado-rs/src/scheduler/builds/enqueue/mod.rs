//! The enqueue half of one poll pass: one recipe, one fenced registry
//! generation, one build job per platform the recipe names.

pub(super) mod claimability;

use std::collections::BTreeMap;

use serde_json::Value;

use crate::queue::submit::{default_store, stable_run_id, submit_batch, SubmitOptions};
use crate::targets::{
    fleet_namespace_mismatch, platform_job_os_arch, BuildRecipe, BuildRun, Registry, RegistryStore,
};

use super::command::build_job_command;
use super::watch::ls_remote;
use claimability::Claimability;

/// One recipe: resolve the remote sha, and when it is new, submit one build
/// job per platform the recipe names and record them through the registry
/// compare-and-swap fence. The fenced re-read (not the coordinator's cached
/// copy) decides whether the sha is actually new, so a stale registry cache
/// cannot double-submit.
///
/// The write edits the RAW document (declare-version pattern): only the one
/// recipe entry's `last_seen_ref`/`runs` keys change. Round-tripping the
/// whole document through the typed `Registry` would emit nulls the registry
/// validator rejects and drop entries the lenient loader skips.
///
/// `runs` is merged, not replaced: a platform this pass did not submit for
/// keeps the run the registry already recorded for it, including whatever the
/// completion pass wrote there.
///
/// Two gates run before any submission, both against the freshly fenced
/// document, because both failure modes used to be silent:
///
/// * [`fleet_namespace_mismatch`] — jobs land in the queue namespace THIS
///   machine's config resolves, so a coordinator whose ambient namespace is
///   not the fleet's would enqueue builds no fleet worker can ever see.
/// * [`Claimability`] — a platform with no live worker gets no job and an
///   `unclaimable` run carrying the reason, instead of a job that sits in
///   the queue forever. The sha stays unseen for that recipe, so the pass
///   resubmits the moment a worker comes back.
pub(super) async fn poll_one(
    registry: &Registry,
    recipe: &BuildRecipe,
    log: &dyn Fn(&str),
) -> Result<(), String> {
    let sha = ls_remote(&recipe.repo, &recipe.branch).await?;
    if recipe.last_seen_ref.as_deref() == Some(sha.as_str()) {
        return Ok(());
    }
    let store = RegistryStore::open()
        .await
        .map_err(|exc| format!("registry store open failed: {exc}"))?;
    let versioned = store
        .read_versioned()
        .await
        .map_err(|exc| format!("registry read failed: {exc}"))?
        .ok_or_else(|| format!("no registry document at {}", store.location()))?;
    let mut document: Value = serde_json::from_str(&versioned.content)
        .map_err(|exc| format!("registry parse failed: {exc}"))?;
    if let Some(mismatch) = fleet_namespace_mismatch(&document) {
        return Err(mismatch);
    }
    let Some(entry) = document
        .get_mut("builds")
        .and_then(Value::as_array_mut)
        .and_then(|entries| {
            entries
                .iter_mut()
                .find(|entry| entry.get("name").and_then(Value::as_str) == Some(&recipe.name))
        })
    else {
        return Ok(()); // removed since the cached read; nothing to build
    };
    // Fresh-document re-check: a concurrent writer may have disabled the
    // recipe or recorded this sha since the cached read.
    let fresh: BuildRecipe = serde_json::from_value(entry.clone())
        .map_err(|exc| format!("recipe entry no longer parses: {exc}"))?;
    if !fresh.enabled || fresh.last_seen_ref.as_deref() == Some(sha.as_str()) {
        return Ok(());
    }
    if fresh.platforms.is_empty() {
        return Err(format!(
            "{} moved to {sha} but the recipe's `platforms` list is empty, so there \
             is no machine to build it on",
            fresh.branch
        ));
    }
    let command = build_job_command(&fresh);
    let at = crate::models::isoformat_utc(chrono::Utc::now());
    let queue = default_store(crate::config::bucket())
        .await
        .map_err(|exc| format!("queue store open failed: {exc}"))?;
    let claimability = Claimability::read(registry, &queue).await?;
    let mut runs: BTreeMap<String, BuildRun> = fresh.runs.clone();
    let mut submitted: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut unclaimable: Vec<String> = Vec::new();
    for platform in &fresh.platforms {
        let Some((platform_os, architecture)) = platform_job_os_arch(platform) else {
            // A word that names no machine is reported and skipped, never
            // retried: it will not have become a platform by the next pass,
            // and holding the sha back for it would rebuild every other
            // platform of this recipe on every pass, forever.
            log(&format!(
                "build {}: platform {platform:?} is not a release platform ({}); skipped",
                fresh.name,
                crate::deploy::products::PLATFORMS.join(", ")
            ));
            continue;
        };
        if let Some(reason) = claimability.refusal(registry, platform) {
            runs.insert(
                platform.clone(),
                BuildRun {
                    status: "unclaimable".to_string(),
                    at: at.clone(),
                    job_id: String::new(),
                    artifact_uris: Vec::new(),
                    version: None,
                    declared: false,
                    reason: Some(reason.clone()),
                },
            );
            unclaimable.push(format!("{platform}: {reason}"));
            continue;
        }
        let options = SubmitOptions {
            run_id: stable_run_id(
                "build-scheduler",
                &format!("{}\0{sha}\0{platform}", fresh.name),
            ),
            platform_os: platform_os.to_string(),
            architecture: architecture.to_string(),
            ..SubmitOptions::default()
        };
        match submit_batch(std::slice::from_ref(&command), &options).await {
            Ok(mut jobs) => {
                let Some(job) = jobs.pop() else {
                    failures.push(format!("{platform}: durable submission returned no job"));
                    continue;
                };
                submitted.push(format!("{platform} job {}", job.job_id));
                runs.insert(
                    platform.clone(),
                    BuildRun {
                        status: "running".to_string(),
                        at: at.clone(),
                        job_id: job.job_id,
                        artifact_uris: Vec::new(),
                        version: None,
                        declared: false,
                        reason: None,
                    },
                );
            }
            Err(exc) => failures.push(format!("{platform}: {exc}")),
        }
    }
    // An unclaimable marker is written once per TRANSITION, not every pass:
    // while the fleet stays dead the reason does not change, and a fenced
    // registry rewrite every cadence buys nothing over the log line this
    // pass already emits. New submissions always write.
    let markers_changed = unclaimable.iter().any(|entry| {
        let platform = entry.split(':').next().unwrap_or_default();
        fresh.runs.get(platform).map(|run| run.status.as_str()) != Some("unclaimable")
    });
    let reasons: Vec<String> = failures.iter().chain(unclaimable.iter()).cloned().collect();
    if submitted.is_empty() && !markers_changed {
        return Err(format!(
            "{} moved to {sha} but no build job was submitted: {}",
            fresh.branch,
            if reasons.is_empty() {
                "no platform the recipe names is a release platform".to_string()
            } else {
                reasons.join("; ")
            }
        ));
    }
    let object = entry
        .as_object_mut()
        .ok_or_else(|| "recipe entry is not an object".to_string())?;
    object.insert(
        "runs".to_string(),
        serde_json::to_value(&runs).map_err(|exc| format!("runs serialize failed: {exc}"))?,
    );
    // The pre-platform shape this build no longer models. Leaving it behind
    // keeps a "last run" in the document that nothing updates again, next to
    // the per-platform runs that are now the record.
    object.remove("last_run");
    // The sha is seen only once every named platform has a job. An
    // unclaimable or failed platform leaves it unseen so the next pass
    // resubmits: a sha marked seen with one platform missing is a commit
    // the fleet believes it built for a machine it never did.
    if reasons.is_empty() {
        object.insert("last_seen_ref".to_string(), Value::String(sha.clone()));
    }
    let payload = format!(
        "{}\n",
        serde_json::to_string_pretty(&document)
            .map_err(|exc| format!("registry serialize failed: {exc}"))?
    );
    let jobs = submitted.join(", ");
    match store.compare_and_swap(&versioned.version, &payload).await {
        Ok(_) => log(&format!(
            "build {}: {} moved to {sha}; submitted {jobs}",
            recipe.name, recipe.branch
        )),
        Err(exc) => log(&format!(
            "build {}: submitted {jobs} but registry update lost a concurrent write \
             (next pass reconciles): {exc}",
            recipe.name
        )),
    }
    if submitted.is_empty() {
        return Err(format!(
            "{} moved to {sha} but no build job was submitted: {}",
            fresh.branch,
            reasons.join("; ")
        ));
    }
    if !reasons.is_empty() {
        log(&format!(
            "build {}: no job for {} — {sha} stays unseen, so the next pass retries \
             the recipe and rebuilds the platforms that did submit",
            recipe.name,
            reasons.join("; ")
        ));
    }
    Ok(())
}
