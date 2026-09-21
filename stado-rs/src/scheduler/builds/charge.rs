//! The one place a compile is charged to the fleet's daily build budget.
//!
//! The ceiling used to be asked by each caller that knew it was submitting a
//! build: the recipe poller, `stado builds run`, and the release pipeline.
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

/// How many of `commands` compile.
pub fn compiling(commands: &[String]) -> usize {
    commands.iter().filter(|command| compiles(command)).count()
}

/// Refuse or charge `wanted` compiles for submission `key`, under the
/// registry's own fence.
///
/// Read, refuse, record and write in one generation: two submissions racing
/// cannot spend the same allowance, and a submission that is refused writes
/// nothing. `key` is the submission's run id, and a key already charged today
/// costs nothing again — the client that submits a build and the worker that
/// claims it both come through here, and the day owes one charge for one
/// build.
pub async fn charge(key: &str, wanted: usize, asked_by: &str) -> Result<(), String> {
    if wanted == 0 {
        return Ok(());
    }
    let now = chrono::Utc::now();
    let (mut document, generation) = crate::cli::registry::fetch_versioned_document()
        .await
        .map_err(|error| format!("reading the fleet's build budget: {error}"))?;
    let budget = BuildBudget::read(&document, now);
    if budget.already_charged(key) {
        return Ok(());
    }
    if let Some(refusal) = budget.refusal(wanted, asked_by) {
        return Err(refusal);
    }
    budget.record(&mut document, wanted, &[key.to_string()]);
    crate::cli::registry::push_document_if(&document, &generation)
        .await
        .map_err(|error| format!("recording {wanted} build(s) against today's budget: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A recipe build and a release build are both compiles; a job that runs
    /// a gate, a delivery or an agent is not, and charging those would make
    /// the ceiling refuse work that occupies no builder.
    #[test]
    fn a_compile_is_recognised_by_what_it_runs() {
        let recipe = format!(
            "set -eu; git clone --depth 1 -- https://example/repo \"$src/checkout\"; \
             cargo build --release; printf '%s' \"$version\" > {BUILD_VERSION_FILE}"
        );
        assert!(compiles(&recipe));
        assert!(compiles(
            "$HOME/.stado/bin/stado release worker --request s3://runs/x/requests/darwin.json"
        ));
        assert!(!compiles(
            "$HOME/.stado/bin/stado release delivery-worker --run 0.21.41"
        ));
        assert!(!compiles("/usr/bin/tar -xzf release.tar.gz && exec ./run"));
        assert_eq!(
            compiling(&[
                "echo one".to_string(),
                format!("printf x > {BUILD_VERSION_FILE}"),
            ]),
            1
        );
    }

    /// A document with a spent day refuses, and the sentence says who asked.
    #[test]
    fn a_spent_day_refuses_the_asker_by_name() {
        let document: serde_json::Value =
            serde_json::json!({ "build_budget": { "day": "2026-09-21", "used": 3, "limit": 3 } });
        let now = chrono::DateTime::parse_from_rfc3339("2026-09-21T12:00:00Z")
            .expect("a test instant")
            .with_timezone(&chrono::Utc);
        let budget = BuildBudget::read(&document, now);
        let refusal = budget
            .refusal(1, "a queue submission")
            .expect("a spent day refuses");
        assert!(refusal.contains("a queue submission"), "{refusal}");
        assert!(refusal.contains("stado queue budget"), "{refusal}");
    }
}
