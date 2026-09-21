//! `stado builds run`: the durable build jobs a run-now enqueues, one per
//! declared platform, and the runs they are recorded as.

use serde_json::{json, Map, Value};

use crate::cli::builds::declaration::canonical_platforms;
use crate::cli::builds::{
    builds_array, fetch_mutation_document, find_entry, normalized_recipe_json, print_json,
};
use crate::cli::CmdError;
use crate::models::isoformat_utc;
use crate::queue::submit::{stable_run_id, submit_batch, SubmitOptions};
use crate::targets::{platform_job_os_arch, BuildRecipe, BuildRun};

/// Enqueue one build job per declared platform now, poll cadence and enable
/// flag notwithstanding — `run` is the operator saying "build it", and saying
/// it about a disabled recipe is how a recipe is vetted before it is enabled.
///
/// The jobs are submitted before the registry write, and each carries the
/// platform as `platform_os`/`architecture` so only a worker of that
/// platform can claim it. A submit that fails leaves the recipe untouched.
pub(super) async fn run_now(name: &str, retry_token: &str, json: bool) -> Result<(), CmdError> {
    if retry_token.trim().is_empty() {
        return Err(CmdError::click("--run-id must not be empty"));
    }
    let (mut document, generation) = fetch_mutation_document().await?;
    let (submitted, updated) =
        submit_recipe_build(&mut document, name, retry_token, "stado builds run").await?;
    crate::cli::registry::push_document_if(&document, &generation).await?;
    if json {
        let jobs: Map<String, Value> = submitted
            .iter()
            .map(|(platform, run)| (platform.clone(), Value::String(run.job_id.clone())))
            .collect();
        return print_json(&json!({
            "name": name,
            "platforms": submitted.iter().map(|(platform, _)| platform).collect::<Vec<_>>(),
            "jobs": jobs,
            "recipe": updated,
        }));
    }
    for (platform, run) in &submitted {
        println!("{name}: submitted build job {} for {platform}", run.job_id);
    }
    Ok(())
}

/// Submit one build job per platform for a recipe into `document`, and record
/// them as its runs. The caller owns the fence: it read the document and it
/// writes it, so a qualification pass records its own rows in the same
/// generation that records these runs.
///
/// `asked_by` is the surface the refusal names when the fleet's daily ceiling
/// is spent, because a refusal that names no asker is one nobody can trace.
pub(crate) async fn submit_recipe_build(
    document: &mut Value,
    name: &str,
    retry_token: &str,
    asked_by: &str,
) -> Result<(Vec<(String, BuildRun)>, Value), CmdError> {
    let recipe: BuildRecipe = {
        let entry = find_entry(builds_array(document)?, name)?;
        serde_json::from_value(entry.clone()).map_err(|error| {
            CmdError::click(format!("build recipe {name:?} does not parse: {error}"))
        })?
    };
    let platforms = canonical_platforms(&recipe.platforms).map_err(|error| {
        CmdError::click(format!(
            "build recipe {name:?} declares no usable platform ({error}); re-add it with --platform"
        ))
    })?;
    // The refusal comes first so the sentence names the recipe rather than
    // the generic submission, but the charge itself belongs to the queue:
    // `submit_batch` charges every command that compiles, so a path that
    // forgot to ask still pays.
    let now = chrono::Utc::now();
    let budget = crate::scheduler::builds::BuildBudget::read(document, now);
    if let Some(refusal) = budget.refusal(platforms.len(), asked_by) {
        return Err(CmdError::click(refusal));
    }
    let command = crate::scheduler::builds::build_job_command(&recipe);
    let at = isoformat_utc(now);
    let mut submitted: Vec<(String, BuildRun)> = Vec::with_capacity(platforms.len());
    for platform in &platforms {
        let (platform_os, architecture) = platform_job_os_arch(platform).ok_or_else(|| {
            CmdError::click(format!("{platform:?} names no job platform/architecture"))
        })?;
        let options = SubmitOptions {
            run_id: stable_run_id(
                crate::scheduler::builds::MANUAL_RUN_PREFIX,
                &format!("{name}\0{retry_token}\0{platform}"),
            ),
            platform_os: platform_os.to_string(),
            architecture: architecture.to_string(),
            ..SubmitOptions::default()
        };
        let mut platform_jobs = submit_batch(std::slice::from_ref(&command), &options).await?;
        let job = platform_jobs
            .pop()
            .ok_or_else(|| CmdError::click("durable build submission returned no job"))?;
        submitted.push((
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
        ));
    }
    let entry = find_entry(builds_array(document)?, name)?;
    let object = entry
        .as_object_mut()
        .expect("a named recipe entry is an object");
    let runs = object
        .entry("runs".to_string())
        .or_insert_with(|| Value::Object(Map::new()))
        .as_object_mut()
        .ok_or_else(|| CmdError::click(format!("build recipe {name:?}: runs is not an object")))?;
    for (platform, run) in &submitted {
        runs.insert(platform.clone(), serde_json::to_value(run)?);
    }
    let updated = normalized_recipe_json(entry);
    Ok((submitted, updated))
}

/// `stado builds budget`: what the fleet has spent on builds today, and the
/// ceiling it is measured against.
///
/// Reading it is the answer to "can I build now"; declaring a ceiling is a
/// deliberate, recorded act, which is the difference between a limit the
/// product holds and one somebody remembers.
pub(crate) async fn budget(limit: Option<u64>, json: bool) -> Result<(), CmdError> {
    let now = chrono::Utc::now();
    let (mut document, generation) = fetch_mutation_document().await?;
    let current = crate::scheduler::builds::BuildBudget::read(&document, now);
    let budget = match limit {
        Some(limit) => {
            current.with_limit(&mut document, limit);
            crate::cli::registry::push_document_if(&document, &generation).await?;
            crate::scheduler::builds::BuildBudget::read(&document, now)
        }
        None => current,
    };
    if json {
        return print_json(&json!({
            "day": budget.day,
            "used": budget.used,
            "limit": budget.limit,
            "remaining": budget.remaining(),
        }));
    }
    println!(
        "build budget {}: {} of {} build job(s) used, {} left",
        budget.day,
        budget.used,
        budget.limit,
        budget.remaining()
    );
    if budget.remaining() == 0 {
        println!(
            "every further build is refused until the count resets at midnight UTC; \
             raise the ceiling deliberately with `stado builds budget --limit <N>`"
        );
    }
    Ok(())
}
