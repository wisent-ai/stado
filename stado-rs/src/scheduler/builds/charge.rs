//! The one place a compile is charged to the fleet's daily build budget.
//!
//! The ceiling used to be asked by each caller that knew it was submitting a
//! build: a recipe poller, a run-now command, and the release pipeline.
//! Every one of them asked, and the fleet still started six builds in an hour
//! against a ceiling of three on 2026-09-21, because the count is only ever
//! as good as the paths that remember to ask. A path nobody updated —
//! `stado job rerun`, a raw `stado submit` carrying a build command, a client
//! built before the ceiling existed — spends the day and leaves the counter
//! saying nothing was spent.
//!
//! So the charge moved to the submission itself. Every job reaches the queue
//! through `submit_batch`, and a command that makes a machine compile is
//! charged there, whoever asked and whatever they knew. The recipes and the
//! poller that used to ask first are gone; the release pipeline still asks,
//! because its refusal names the release, and the charge below is what makes
//! the number true.

use super::approval::{self, BuildIntent};
use super::budget::BuildBudget;

/// What a release pipeline's build job runs on the builder.
const RELEASE_BUILD_PROGRAM: &str = "release worker --request";

/// The file a build job writes beside its artifacts to record the exact tag
/// it built. Recipe builds wrote it; it is still the mark that says a queued
/// command compiles, so a client of any vintage submitting one is charged.
pub const BUILD_VERSION_FILE: &str = "stado-build-version.txt";

/// Whether this command makes a machine compile something for the fleet.
///
/// Two shapes reach the queue: a build that clones a repository and writes
/// [`BUILD_VERSION_FILE`] beside the artifacts it uploads, and a release
/// build, which runs the release worker against a saved request. Both occupy
/// a builder for minutes and both are what the ceiling is about.
pub fn compiles(command: &str) -> bool {
    command.contains(BUILD_VERSION_FILE) || command.contains(RELEASE_BUILD_PROGRAM)
}

/// Refuse or charge the compiles among `commands` for submission `key`,
/// under the registry's own fence.
///
/// Read, refuse, record and write in one generation: two submissions racing
/// cannot spend the same allowance, and a submission that is refused writes
/// nothing. `key` is the submission's run id, and a key already charged today
/// costs nothing again — the client that submits a build and the worker that
/// claims it both come through here, and the day owes one charge for one
/// build. An operator approval covers exactly one build, so it is consulted
/// only when a single command compiles.
pub async fn charge(
    key: &str,
    commands: &[String],
    asked_by: &str,
    intent: Option<&BuildIntent<'_>>,
) -> Result<(), String> {
    let builds: Vec<&String> = commands.iter().filter(|command| compiles(command)).collect();
    if builds.is_empty() {
        return Ok(());
    }
    let wanted = builds.len();
    let now = chrono::Utc::now();
    let (mut document, generation) = crate::cli::registry::fetch_versioned_document()
        .await
        .map_err(|error| format!("reading the fleet's build budget: {error}"))?;
    let budget = BuildBudget::read(&document, now);
    if budget.already_charged(key) || approval::charged(&document, key) {
        return Ok(());
    }
    let approved = if let Some(refusal) = budget.refusal(wanted, asked_by) {
        let Some(intent) = intent.filter(|_| matches!(builds.as_slice(), [_])) else {
            return Err(refusal);
        };
        Some(
            approval::verify(intent)
                .await
                .map_err(|error| format!("{refusal}\n{error}"))?,
        )
    } else {
        None
    };
    budget.record(&mut document, wanted, &[key.to_string()]);
    if let (Some(entry), Some(intent)) = (approved, intent) {
        approval::record(&mut document, entry, intent.platform, key)?;
    }
    crate::cli::registry::push_document_if(&document, &generation)
        .await
        .map_err(|error| format!("recording {wanted} build(s) against today's budget: {error}"))?;
    Ok(())
}
