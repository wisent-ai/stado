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
//! Two gates keep a submission from going silently nowhere. The fleet
//! namespace pin ([`crate::targets::fleet_namespace_mismatch`]) refuses the
//! pass when this machine's ambient queue namespace is not the one the
//! registry records for the fleet — jobs land where the SUBMITTER's config
//! points, so a misconfigured writer and the fleet's readers would otherwise
//! address two queues through one API. The claimability check
//! ([`Claimability`]) refuses a platform no live worker can claim, records
//! the run `unclaimable` with the reason and leaves the sha unseen, so the
//! build happens the moment a worker comes back. Runs that do submit are
//! supervised at completion ([`reconcile_build_runs`]): unclaimed past ten
//! minutes, running past the sixty-minute ceiling, or vanished from the
//! queue becomes `failed` with the diagnosis in the run's `reason`.
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

mod command;
mod enqueue;
mod receipts;
mod watch;

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

use crate::targets::{fetch_registry_remote, read_build_recipes, BUILDS_DISABLED_KEY};

pub use command::{build_job_command, BUILD_VERSION_FILE};
pub use enqueue::claimability::Claimability;

use enqueue::poll_one;
use receipts::reconcile_build_runs;
use watch::recipe_due;

/// Floor between two poll passes, regardless of the coordinator's tick
/// cadence (the local control plane ticks every few seconds).
const PASS_INTERVAL_SECONDS: u64 = 60;

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
        if let Err(error) = poll_one(&registry, recipe, log).await {
            log(&format!("build {}: {error}", recipe.name));
        }
    }
}
