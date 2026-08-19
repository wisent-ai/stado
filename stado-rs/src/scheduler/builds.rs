//! Native-build poller: watch registry build recipes for new commits and
//! enqueue one build job per new ref and platform.
//!
//! Recipes live in the canonical registry's top-level `builds` key
//! (`targets::read_build_recipes`) and are managed by `stado builds`. The
//! coordinator tick loop calls
//! [`poll_build_recipes`] after every `run_tick`; the poller self-rate-limits
//! to one pass per [`PASS_INTERVAL_SECONDS`], and each recipe additionally
//! honours its own `interval_seconds` cadence, so a short coordinator tick
//! never turns into a `git ls-remote` storm.
//!
//! A pass is: `git ls-remote <repo> <ref>` per due, enabled recipe; on a sha
//! the registry has not seen, submit ONE job PER PLATFORM the recipe names
//! through the existing queue submit path with the command
//! [`build_job_command`] generates — each job declaring the `platform_os` and
//! `architecture` that only a host of that platform will claim
//! (`providers::local::helpers::job_eligible`) — then record `last_seen_ref`
//! and a `running` entry in `runs[<platform>]` with a surgical raw-document
//! edit through the registry compare-and-swap fence (the declare-version
//! pattern). A CAS conflict is logged and skipped — the write
//! lost to a concurrent registry edit, and the next pass re-reads and
//! reconciles. Any per-recipe failure is one log line, never a panic: builds
//! are additive and must not destabilize the scheduling tick.
//!
//! A sha is recorded as seen only once every named platform has a job. A
//! platform whose submit failed leaves `last_seen_ref` alone so the next pass
//! retries the recipe, because a sha marked seen with one platform missing is
//! a commit the fleet believes it built for a machine it never did.
//!
//! Fleet-wide kill switch: a top-level `builds_disabled: true` in the
//! registry document halts polling entirely.
//!
//! Boundary: a build produces job results (artifacts under the job's
//! canonical `status/<job_id>/output/` prefix, including the built commit's
//! exact tag in [`BUILD_VERSION_FILE`]) and, for an `auto_declare` recipe, a
//! managed-version declaration for the hosts on that platform. It NEVER
//! writes `release_control.products`: promoting a signed release verifies a
//! manifest and its signature (`stado release promote`) and stays a
//! deliberate, separate step.

use std::collections::{BTreeMap, HashMap};
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::deploy::host_release::is_exact_semver;
use crate::deploy::products;
use crate::models::isoformat_utc;
use crate::queue::runs::TERMINAL_PREFIXES;
use crate::queue::storage::JobStorage;
use crate::queue::submit::{default_store, submit_job, SubmitOptions};
use crate::targets::{
    fetch_registry_remote, platform_job_os_arch, read_build_recipes, BuildRecipe, BuildRun,
    Registry, RegistryStore, BUILDS_DISABLED_KEY, BUILDS_KEY,
};

/// Floor between two poll passes, regardless of the coordinator's tick
/// cadence (the local control plane ticks every few seconds).
const PASS_INTERVAL_SECONDS: u64 = 60;

/// `git ls-remote` wall-clock budget. A wedged remote (credential prompt,
/// dead host) must cost one recipe one line, not stall the tick daemon.
const LS_REMOTE_TIMEOUT_SECONDS: u64 = 30;


/// Poll bookkeeping. Process-local by design: `last_seen_ref` in the
/// registry is the durable dedup record; these instants only pace work.
struct PollState {
    last_pass: Option<Instant>,
    last_recipe_poll: HashMap<String, Instant>,
}

static POLL_STATE: LazyLock<Mutex<PollState>> = LazyLock::new(|| {
    Mutex::new(PollState {
        last_pass: None,
        last_recipe_poll: HashMap::new(),
    })
});

/// POSIX single-quote `value` so operator-entered recipe fields (repo, ref,
/// command, artifact paths) can never terminate or extend the generated
/// shell chain.
fn sh_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

/// Name of the file every build writes at the root of its uploaded output:
/// the exact tag the built commit carries, or nothing at all.
///
/// The version of a build is a property of the commit, and only the machine
/// holding the checkout can read it. Resolving it later from the recipe's
/// branch answers for whatever the branch points at by then, which is how a
/// build of one commit gets declared as the version of another.
pub const BUILD_VERSION_FILE: &str = "stado-build-version.txt";

/// The full command a build job runs, from the agent's job workdir: shallow
/// single-branch clone into a self-cleaning temp dir, the built commit's exact
/// tag recorded in [`BUILD_VERSION_FILE`], the recipe's build command inside
/// the checkout, then each declared artifact copied (tree structure
/// preserved) into the workdir's `output/` — the directory the agent already
/// uploads under the job's canonical results prefix
/// (`status/<job_id>/output/`, plus any `output_uri` mirror).
///
/// The tag is read before the build runs: a build command is free to check
/// out, fetch or tag inside the checkout, and the version being recorded is
/// the one that was cloned. `git describe --exact-match` prints nothing and
/// fails for an untagged commit, which is most commits — the redirect still
/// creates the file, and an empty file is the answer "this commit carries no
/// version" rather than a missing upload nobody can distinguish from a build
/// that never got that far.
///
/// Shared with `stado builds run`, so the operator's manual enqueue and the
/// poller submit byte-identical commands.
pub fn build_job_command(recipe: &BuildRecipe) -> String {
    let mut command = format!(
        "set -eu; root=\"$PWD\"; src=\"$(mktemp -d)\"; trap 'rm -rf \"$src\"' EXIT; \
         git clone --depth 1 --branch {branch} -- {repo} \"$src/checkout\"; \
         cd \"$src/checkout\"; mkdir -p \"$root/output\"; \
         {{ git describe --exact-match --tags HEAD 2>/dev/null || :; }} \
         > \"$root/output/{version}\"; \
         sh -c {build}",
        branch = sh_quote(&recipe.branch),
        repo = sh_quote(&recipe.repo),
        version = BUILD_VERSION_FILE,
        build = sh_quote(&recipe.command),
    );
    for artifact in &recipe.artifacts {
        let quoted = sh_quote(artifact);
        command.push_str(&format!(
            "; mkdir -p \"$(dirname \"$root/output/\"{quoted})\"; \
             cp -R {quoted} \"$root/output/\"{quoted}"
        ));
    }
    command
}

/// `git ls-remote <repo> <ref>` -> the remote sha, under a hard timeout and
/// with credential prompts disabled (an unauthenticated private repo must
/// fail, not hang).
async fn ls_remote(repo: &str, branch: &str) -> Result<String, String> {
    let mut command = tokio::process::Command::new("git");
    command
        .args(["ls-remote", "--", repo, branch])
        .env("GIT_TERMINAL_PROMPT", "0")
        .stdin(std::process::Stdio::null());
    let output = tokio::time::timeout(
        Duration::from_secs(LS_REMOTE_TIMEOUT_SECONDS),
        command.output(),
    )
    .await
    .map_err(|_| format!("git ls-remote timed out after {LS_REMOTE_TIMEOUT_SECONDS}s"))?
    .map_err(|exc| format!("git ls-remote spawn failed: {exc}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!(
            "git ls-remote failed: {}",
            stderr.trim().lines().next().unwrap_or("(no stderr)")
        ));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let sha = stdout
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().next())
        .unwrap_or("")
        .to_string();
    if sha.len() != 40 || !sha.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err(format!("ref {branch:?} not found on remote"));
    }
    Ok(sha)
}

/// True when this recipe's own cadence has elapsed (and stamp the attempt).
/// Stamped before the ls-remote so a failing remote is retried on the
/// recipe's cadence, not on every pass.
fn recipe_due(name: &str, interval_seconds: u64) -> bool {
    let mut state = POLL_STATE.lock().expect("build poll state lock");
    let due = state
        .last_recipe_poll
        .get(name)
        .is_none_or(|last| last.elapsed() >= Duration::from_secs(interval_seconds.max(1)));
    if due {
        state.last_recipe_poll.insert(name.to_string(), Instant::now());
    }
    due
}

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
async fn poll_one(recipe: &BuildRecipe, log: &dyn Fn(&str)) -> Result<(), String> {
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
    let mut runs: BTreeMap<String, BuildRun> = fresh.runs.clone();
    let mut submitted: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
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
        let options = SubmitOptions {
            platform_os: platform_os.to_string(),
            architecture: architecture.to_string(),
            ..SubmitOptions::default()
        };
        match submit_job(&command, &options).await {
            Ok(job) => {
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
                    },
                );
            }
            Err(exc) => failures.push(format!("{platform}: {exc}")),
        }
    }
    if submitted.is_empty() {
        return Err(format!(
            "{} moved to {sha} but no build job was submitted: {}",
            fresh.branch,
            if failures.is_empty() {
                "no platform the recipe names is a release platform".to_string()
            } else {
                failures.join("; ")
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
    if failures.is_empty() {
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
    if !failures.is_empty() {
        log(&format!(
            "build {}: no job for {} — {sha} stays unseen, so the next pass retries \
             the recipe and rebuilds the platforms that did submit",
            recipe.name,
            failures.join("; ")
        ));
    }
    Ok(())
}

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

/// The terminal prefix `job_id` has landed in, or `None` while it is still
/// queued or running — or has been swept out of the queue entirely, which
/// reads the same way here: a run whose job cannot be found is left
/// `running` rather than declared failed on absence.
async fn terminal_prefix(store: &JobStorage, job_id: &str) -> Result<Option<&'static str>, String> {
    for prefix in TERMINAL_PREFIXES {
        let found = store
            .read_job(prefix, job_id)
            .await
            .map_err(|exc| format!("reading {prefix}/{job_id}: {exc}"))?;
        if found.is_some() {
            return Ok(Some(prefix));
        }
    }
    Ok(None)
}

/// The version a finished build recorded for the commit it built: the first
/// line of [`BUILD_VERSION_FILE`] at the root of its uploaded output, with a
/// leading `v` stripped.
///
/// `None` covers every way a build has nothing to declare, and they are all
/// ordinary: the commit carried no tag (the file is empty), the tag is not an
/// exact semantic version, or the job never uploaded the file at all. Only
/// the middle case says anything an operator did not ask for, so only it
/// logs.
async fn recorded_version(store: &JobStorage, job_id: &str, log: &dyn Fn(&str)) -> Option<String> {
    let path = format!("status/{job_id}/output/{BUILD_VERSION_FILE}");
    let text = match store.download_text(&path).await {
        Ok(Some(text)) => text,
        Ok(None) => return None,
        Err(exc) => {
            log(&format!("build job {job_id}: reading {path}: {exc}"));
            return None;
        }
    };
    let tag = text.lines().next().unwrap_or_default().trim();
    let version = tag.strip_prefix('v').unwrap_or(tag);
    if version.is_empty() {
        return None;
    }
    if !is_exact_semver(version) {
        log(&format!(
            "build job {job_id}: tag {tag:?} is not an exact semantic version; \
             the run records no version"
        ));
        return None;
    }
    Some(version.to_string())
}

/// Every blob the job uploaded under its canonical results prefix, except
/// the version file: that one is the run's own bookkeeping, recorded as
/// [`BuildRun::version`], and listing it as an artifact would offer the
/// fleet a text file as a build output.
async fn uploaded_artifacts(store: &JobStorage, job_id: &str, log: &dyn Fn(&str)) -> Vec<String> {
    let prefix = format!("status/{job_id}/output/");
    let version_file = format!("{prefix}{BUILD_VERSION_FILE}");
    match store.list_paths(&prefix, 0).await {
        Ok(paths) => paths
            .into_iter()
            .filter(|path| path.len() > prefix.len() && *path != version_file)
            .collect(),
        Err(exc) => {
            log(&format!("build job {job_id}: listing {prefix}: {exc}"));
            Vec::new()
        }
    }
}

/// Declare `version` as the managed version of the recipe's product on every
/// registry host of `platform`, and say so once per host.
///
/// This calls [`crate::cli::host::declare_version`] itself — the same
/// function `stado host declare-version` runs, fence and validation included,
/// which also means it prints the CLI's own confirmation line per host
/// alongside the daemon log line. A recipe declares the product its NAME
/// selects: a recipe named after nothing the fleet declares has artifacts to
/// keep and no product to move, and says so instead of guessing.
///
/// The return value is what [`BuildRun::declared`] records, so it is true
/// only when every matching host took the declaration: a partial fleet is
/// not a declared version.
async fn declare_on_platform(
    registry: &Registry,
    recipe: &str,
    platform: &str,
    version: &str,
    log: &dyn Fn(&str),
) -> bool {
    let product = match products::product(recipe) {
        Ok(product) => product,
        Err(exc) => {
            log(&format!(
                "build {recipe}: auto-declare skipped for {platform}: {exc}"
            ));
            return false;
        }
    };
    if !product.platforms.iter().any(|word| word == platform) {
        log(&format!(
            "build {recipe}: auto-declare skipped for {platform}: {} is not published for it",
            product.name
        ));
        return false;
    }
    let hosts: Vec<String> = registry
        .targets
        .iter()
        .filter(|target| target.release_platform == platform)
        .map(|target| target.name.clone())
        .collect();
    if hosts.is_empty() {
        log(&format!(
            "build {recipe}: auto-declare skipped for {platform}: no registry host reports it"
        ));
        return false;
    }
    let mut declared_everywhere = true;
    for host in &hosts {
        match crate::cli::host::declare_version(host, &product.name, version, false).await {
            Ok(()) => log(&format!(
                "build {recipe}: declared {} {version} on {host} ({platform})",
                product.name
            )),
            Err(exc) => {
                declared_everywhere = false;
                log(&format!(
                    "build {recipe}: declaring {} {version} on {host} ({platform}) failed: {exc}",
                    product.name
                ));
            }
        }
    }
    declared_everywhere
}

/// One completion pass: every recorded run still marked `running` whose job
/// has reached a terminal prefix becomes a recorded outcome — succeeded or
/// failed, the version the build wrote down, the artifacts it uploaded — and,
/// for an `auto_declare` recipe with a version, a managed-version
/// declaration on that platform's hosts.
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
async fn reconcile_build_runs(registry: &Registry, declare_allowed: bool, log: &dyn Fn(&str)) {
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
        let prefix = match terminal_prefix(&store, &run.job_id).await {
            Ok(Some(prefix)) => prefix,
            Ok(None) => continue,
            Err(exc) => {
                log(&format!("build {}: {exc}", recipe.name));
                continue;
            }
        };
        let succeeded = prefix == "completed" || prefix == "uploaded";
        let mut updated = BuildRun {
            status: if succeeded { "succeeded" } else { "failed" }.to_string(),
            at: isoformat_utc(chrono::Utc::now()),
            job_id: run.job_id.clone(),
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
                    "build {}: auto-declare skipped for {platform}: job {} built a commit with no \
                     exact version tag, so there is no version to declare",
                    recipe.name, run.job_id
                )),
                (Some(_), false) => log(&format!(
                    "build {}: auto-declare withheld for {platform}: registry sets \
                     builds_disabled=true",
                    recipe.name
                )),
                (Some(version), true) => {
                    updated.declared =
                        declare_on_platform(registry, &recipe.name, platform, &version, log).await;
                }
            }
        }
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

/// Write every outcome of one pass in a single fenced generation.
///
/// A per-outcome compare-and-swap would have a pass that finished four
/// platforms lose three generations to itself. The write is the raw-document
/// surgical edit the rest of this module uses, and it replaces a platform's
/// run only when the job id still matches the one that was reconciled: a
/// concurrent `builds run` for the same platform is a NEWER job, and stamping
/// a finished job's outcome over it would lose the submission.
async fn commit_run_outcomes(outcomes: &[RunOutcome]) -> Result<(), String> {
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
    let Some(entries) = document.get_mut(BUILDS_KEY).and_then(Value::as_array_mut) else {
        return Ok(()); // every recipe removed since the cached read
    };
    let mut written = 0usize;
    for outcome in outcomes {
        let Some(entry) = entries.iter_mut().find(|entry| {
            entry.get("name").and_then(Value::as_str) == Some(outcome.recipe.as_str())
        }) else {
            continue; // recipe removed while its build finished
        };
        let Some(object) = entry.as_object_mut() else {
            continue;
        };
        let runs = object
            .entry("runs".to_string())
            .or_insert_with(|| Value::Object(serde_json::Map::new()))
            .as_object_mut()
            .ok_or_else(|| format!("build recipe {:?}: runs is not an object", outcome.recipe))?;
        let recorded = runs
            .get(&outcome.platform)
            .and_then(|run| run.get("job_id"))
            .and_then(Value::as_str);
        if recorded != Some(outcome.run.job_id.as_str()) {
            continue;
        }
        runs.insert(
            outcome.platform.clone(),
            serde_json::to_value(&outcome.run)
                .map_err(|exc| format!("run serialize failed: {exc}"))?,
        );
        written += 1;
    }
    if written == 0 {
        return Ok(());
    }
    let payload = format!(
        "{}\n",
        serde_json::to_string_pretty(&document)
            .map_err(|exc| format!("registry serialize failed: {exc}"))?
    );
    store
        .compare_and_swap(&versioned.version, &payload)
        .await
        .map_err(|exc| {
            format!("recording {written} run(s) lost a concurrent registry write: {exc}")
        })?;
    Ok(())
}

/// One rate-limited poll pass over every enabled build recipe. Called from
/// the coordinator tick loops after `run_tick`; returns immediately when the
/// pass floor has not elapsed, the kill switch is set, or no recipe is due.
/// Never fails: every error is one log line for its recipe.
pub async fn poll_build_recipes(log: &dyn Fn(&str)) {
    {
        let mut state = POLL_STATE.lock().expect("build poll state lock");
        if state
            .last_pass
            .is_some_and(|last| last.elapsed() < Duration::from_secs(PASS_INTERVAL_SECONDS))
        {
            return;
        }
        state.last_pass = Some(Instant::now());
    }
    // The coordinator's own registry fetch path (short-TTL cached); the
    // fenced re-read in `poll_one` is what actually guards the write.
    let registry = match fetch_registry_remote().await {
        Ok(registry) => registry,
        Err(exc) => {
            log(&format!("build poll skipped: registry unreachable: {exc}"));
            return;
        }
    };
    // A build already submitted still has an outcome to record when the kill
    // switch flips, so the completion pass runs either way; what the switch
    // withholds is the authority to act on it (`declare_allowed`) and any
    // further submission.
    let disabled = registry
        .extra
        .get(BUILDS_DISABLED_KEY)
        .and_then(serde_json::Value::as_bool)
        == Some(true);

    reconcile_build_runs(&registry, !disabled, log).await;

    if disabled {
        log("build poll halted: registry sets builds_disabled=true");
        return;
    }
    let recipes = read_build_recipes(&registry);
    {
        // Drop pacing entries for recipes that no longer exist.
        let mut state = POLL_STATE.lock().expect("build poll state lock");
        state
            .last_recipe_poll
            .retain(|name, _| recipes.iter().any(|recipe| &recipe.name == name));
    }
    for recipe in &recipes {
        if !recipe.enabled || !recipe_due(&recipe.name, recipe.interval_seconds) {
            continue;
        }
        if let Err(error) = poll_one(recipe, log).await {
            log(&format!("build {}: {error}", recipe.name));
        }
    }
}
