//! Native-build poller: watch registry build recipes for new commits and
//! enqueue one build job per new ref.
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
//! the registry has not seen, submit ONE job through the existing queue
//! submit path with the command [`build_job_command`] generates, then record
//! `last_seen_ref` and a `running` `last_run` with a surgical raw-document
//! edit through the registry compare-and-swap fence (the declare-version
//! pattern). A CAS conflict is logged and skipped — the write
//! lost to a concurrent registry edit, and the next pass re-reads and
//! reconciles. Any per-recipe failure is one log line, never a panic: builds
//! are additive and must not destabilize the scheduling tick.
//!
//! Fleet-wide kill switch: a top-level `builds_disabled: true` in the
//! registry document halts polling entirely.
//!
//! v1 boundary: a build produces job results (artifacts under the job's
//! canonical `status/<job_id>/output/` prefix) and nothing more. Version
//! declaration and fleet delivery stay manual (`stado host declare-version`,
//! `converge --apply`).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::queue::submit::{submit_job, SubmitOptions};
use crate::targets::{
    fetch_registry_remote, read_build_recipes, BuildRecipe, BuildRun, RegistryStore,
    BUILDS_DISABLED_KEY,
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

/// The full command a build job runs, from the agent's job workdir: shallow
/// single-branch clone into a self-cleaning temp dir, the recipe's build
/// command inside the checkout, then each declared artifact copied (tree
/// structure preserved) into the workdir's `output/` — the directory the
/// agent already uploads under the job's canonical results prefix
/// (`status/<job_id>/output/`, plus any `output_uri` mirror).
///
/// Shared with `stado builds run`, so the operator's manual enqueue and the
/// poller submit byte-identical commands.
pub fn build_job_command(recipe: &BuildRecipe) -> String {
    let mut command = format!(
        "set -eu; root=\"$PWD\"; src=\"$(mktemp -d)\"; trap 'rm -rf \"$src\"' EXIT; \
         git clone --depth 1 --branch {branch} -- {repo} \"$src/checkout\"; \
         cd \"$src/checkout\"; sh -c {build}",
        branch = sh_quote(&recipe.branch),
        repo = sh_quote(&recipe.repo),
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

/// One recipe: resolve the remote sha, and when it is new, submit the build
/// job and record it through the registry compare-and-swap fence. The fenced
/// re-read (not the coordinator's cached copy) decides whether the sha is
/// actually new, so a stale registry cache cannot double-submit.
///
/// The write edits the RAW document (declare-version pattern): only the one
/// recipe entry's `last_seen_ref`/`last_run` keys change. Round-tripping the
/// whole document through the typed `Registry` would emit nulls the registry
/// validator rejects and drop entries the lenient loader skips.
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
    let command = build_job_command(&fresh);
    let job = submit_job(&command, &SubmitOptions::default())
        .await
        .map_err(|exc| format!("job submit failed: {exc}"))?;
    let run = BuildRun {
        status: "running".to_string(),
        at: crate::models::isoformat_utc(chrono::Utc::now()),
        job_id: job.job_id.clone(),
        artifact_uris: Vec::new(),
    };
    let object = entry
        .as_object_mut()
        .ok_or_else(|| "recipe entry is not an object".to_string())?;
    object.insert("last_seen_ref".to_string(), Value::String(sha.clone()));
    object.insert(
        "last_run".to_string(),
        serde_json::to_value(&run).map_err(|exc| format!("run serialize failed: {exc}"))?,
    );
    let payload = format!(
        "{}\n",
        serde_json::to_string_pretty(&document)
            .map_err(|exc| format!("registry serialize failed: {exc}"))?
    );
    match store.compare_and_swap(&versioned.version, &payload).await {
        Ok(_) => log(&format!(
            "build {}: {} moved to {sha}; submitted job {}",
            recipe.name, recipe.branch, job.job_id
        )),
        Err(exc) => log(&format!(
            "build {}: job {} submitted but registry update lost a concurrent write \
             (next pass reconciles): {exc}",
            recipe.name, job.job_id
        )),
    }
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
    if registry
        .extra
        .get(BUILDS_DISABLED_KEY)
        .and_then(serde_json::Value::as_bool)
        == Some(true)
    {
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
