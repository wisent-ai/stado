//! The completion half of one poll pass: what a submitted build turned into.
//!
//! The seams are the supervision verdicts (`supervision`), the facts a
//! finished job uploaded (`artifacts`), the managed-version declaration an
//! `auto_declare` recipe earns (`declare`), and the single fenced registry
//! write that records the pass (`commit`).

mod artifacts;
mod commit;
mod declare;
mod supervision;

use crate::models::isoformat_utc;
use crate::queue::submit::default_store;
use crate::targets::{read_build_recipes, BuildRecipe, BuildRun, Registry};

use artifacts::{recorded_version, uploaded_artifacts};
use commit::commit_run_outcomes;
use declare::declare_on_platform;
use supervision::{stuck_reason, terminal_prefix};

// ---------------------------------------------------------------------------
// Completion: record what a finished build produced, then declare it
// ---------------------------------------------------------------------------

/// One finished run's terminal facts, resolved from the queue and declared
/// before the registry is touched: the fenced write is a compare-and-swap,
/// and holding storage reads and remote declarations inside it is how a pass
/// loses its own generation.
struct RunOutcome {
    recipe: String,
    platform: String,
    run: BuildRun,
}

/// One completion pass: every recorded run still marked `running` whose job
/// has reached a terminal prefix becomes a recorded outcome — succeeded or
/// failed, the version the build wrote down, the artifacts it uploaded — and,
/// for an `auto_declare` recipe with a version, a managed-version
/// declaration on that platform's hosts. A run whose job has NOT reached a
/// terminal prefix is supervised instead of waited on forever
/// ([`stuck_reason`]): queued past the claim threshold, running past the
/// build ceiling, or vanished from the queue altogether all become `failed`
/// with the diagnosis in the run's `reason`, because a `running` that means
/// "nobody knows" is how a dead fleet reads as a busy one.
///
/// Declaration happens BEFORE the registry write on purpose. `declared` must
/// mean "`managed_versions` says so", and a declaration is idempotent (one
/// key set to one value under its own fence), so a crash between the two
/// costs a repeated declaration on the next pass — never a run claiming a
/// declaration nobody made.
///
/// `declare_allowed` is false while the fleet-wide kill switch is set: the
/// outcome of an already-submitted build is still recorded (a run stuck at
/// `running` forever is the switch corrupting the record), but acting on it
/// is exactly what the switch withholds.
///
/// Never fails: every error is one log line, and the next pass reconciles.
pub(super) async fn reconcile_build_runs(
    registry: &Registry,
    declare_allowed: bool,
    log: &dyn Fn(&str),
) {
    let recipes = read_build_recipes(registry);
    let pending: Vec<(&BuildRecipe, &String, &BuildRun)> = recipes
        .iter()
        .flat_map(|recipe| {
            recipe
                .runs
                .iter()
                .filter(|(_, run)| run.status == "running")
                .map(move |(platform, run)| (recipe, platform, run))
        })
        .collect();
    if pending.is_empty() {
        return;
    }
    let store = match default_store(crate::config::bucket()).await {
        Ok(store) => store,
        Err(exc) => {
            log(&format!(
                "build completion skipped: queue unreachable: {exc}"
            ));
            return;
        }
    };
    let mut outcomes: Vec<RunOutcome> = Vec::new();
    for (recipe, platform, run) in pending {
        let prefix = match terminal_prefix(&store, run).await {
            Ok(prefix) => prefix,
            Err(exc) => {
                log(&format!("build {}: {exc}", recipe.name));
                continue;
            }
        };
        let updated = match prefix {
            Some(prefix) => {
                let succeeded = prefix == "completed" || prefix == "uploaded";
                let mut updated = BuildRun {
                    status: if succeeded { "succeeded" } else { "failed" }.to_string(),
                    at: isoformat_utc(chrono::Utc::now()),
                    job_id: run.job_id.clone(),
                    run_id: run.run_id.clone(),
                    artifact_uris: if succeeded {
                        uploaded_artifacts(&store, &run.job_id, log).await
                    } else {
                        Vec::new()
                    },
                    version: if succeeded {
                        recorded_version(&store, &run.job_id, log).await
                    } else {
                        None
                    },
                    declared: false,
                    reason: None,
                };
                log(&format!(
                    "build {}: {platform} job {} {} ({prefix}), version {}",
                    recipe.name,
                    run.job_id,
                    updated.status,
                    updated.version.as_deref().unwrap_or("none")
                ));
                if succeeded && recipe.auto_declare {
                    match (updated.version.clone(), declare_allowed) {
                        (None, _) => log(&format!(
                            "build {}: auto-declare skipped for {platform}: job {} built a \
                             commit with no exact version tag, so there is no version to declare",
                            recipe.name, run.job_id
                        )),
                        (Some(_), false) => log(&format!(
                            "build {}: auto-declare withheld for {platform}: registry sets \
                             builds_disabled=true",
                            recipe.name
                        )),
                        (Some(version), true) => {
                            updated.declared = declare_on_platform(
                                registry,
                                &recipe.name,
                                platform,
                                &version,
                                log,
                            )
                            .await;
                        }
                    }
                }
                updated
            }
            None => {
                let Some(reason) = stuck_reason(&store, run, log).await else {
                    continue;
                };
                log(&format!(
                    "build {}: {platform} job {} failed — {reason}",
                    recipe.name, run.job_id
                ));
                BuildRun {
                    status: "failed".to_string(),
                    at: isoformat_utc(chrono::Utc::now()),
                    job_id: run.job_id.clone(),
                    run_id: run.run_id.clone(),
                    artifact_uris: Vec::new(),
                    version: None,
                    declared: false,
                    reason: Some(reason),
                }
            }
        };
        outcomes.push(RunOutcome {
            recipe: recipe.name.clone(),
            platform: platform.clone(),
            run: updated,
        });
    }
    if outcomes.is_empty() {
        return;
    }
    if let Err(error) = commit_run_outcomes(&outcomes).await {
        log(&format!("build completion: {error}"));
    }
}
