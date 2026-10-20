//! Where each listed run stands, decided once from its state and legs, and
//! how far a live build has compiled.

use serde_json::Value;

use crate::cli::CmdError;
use crate::queue::storage::JobStorage;
use crate::release_pipeline::ReleaseRunState;

use super::jobs::{compile_progress, Progress};
use super::legs::{published_every_submitted_platform, required_leg_failed};

/// Settle each run's `phase` (and `state`, when a required leg already
/// failed), and measure the compile progress of each running leg of a live
/// run against `older`, the runs the listing did not consume.
pub(super) async fn settle(
    store: &JobStorage,
    runs: &mut [Value],
    older: &[String],
) -> Result<(), CmdError> {
    for run in runs.iter_mut() {
        // Where the run stands, decided once here: failed, published or
        // still going. The desktop console used to re-decide it by matching
        // the state word, which is the same list in a second language.
        let state = run["state"].as_str().and_then(ReleaseRunState::named);
        if let Some(state) = &state {
            run["phase"] = Value::String(state.phase().to_owned());
        }
        // A run replaced after every platform it submitted was published
        // still published those bytes: they are signed, in the store and on
        // the hosts its deliveries reached. Reading it as failed told Oko no
        // release of that source exists.
        if state == Some(ReleaseRunState::Superseded) && published_every_submitted_platform(run) {
            run["phase"] = Value::String("published".to_owned());
        }
        // Whether a failed leg fails the release is submit's rule, not this
        // listing's: only a platform the build manifest marks required does.
        // Every failed leg is weighed — one the run already recorded (a build
        // refresh can adopt a failed leg while the run stays waiting) as well
        // as one whose job was just seen ending — so state and phase agree
        // for the CLI line (which prints `state`), the JSON and the Desktop
        // console (which reads `phase`). The stored word is kept as
        // `recorded_state` for whoever resumes the run.
        let unfinished = state
            .as_ref()
            .is_some_and(|state| !state.finished() && !state.published());
        if unfinished && required_leg_failed(store, run).await {
            let failed = ReleaseRunState::Failed;
            run["recorded_state"] = run["state"].clone();
            run["state"] = serde_json::to_value(&failed).map_err(|cause| {
                CmdError::click(format!("the failed run state does not serialize: {cause}"))
            })?;
            run["phase"] = Value::String(failed.phase().to_owned());
            continue;
        }
        if unfinished {
            measure_running_legs(store, run, older).await;
        }
    }
    Ok(())
}

/// The build's own progress on each running leg of `run`, measured against
/// that platform's previous run ([`compile_progress`]).
async fn measure_running_legs(store: &JobStorage, run: &mut Value, older: &[String]) {
    // A run that names no product has no previous run of its own to be
    // measured against; read as "", it was compared with runs of "" and
    // shown without a denominator as though none had been built.
    let product_name = run["product"].as_str().map(str::to_owned);
    let Some(platforms) = run["platforms"].as_object_mut() else {
        return;
    };
    for (platform_name, record) in platforms.iter_mut() {
        let Some(job_id) = record["job_id"].as_str().map(str::to_owned) else {
            continue;
        };
        if record["job_state"].as_str() != Some("running") {
            continue;
        }
        let measured = compile_progress(
            store,
            &job_id,
            older,
            product_name.as_deref(),
            platform_name,
        )
        .await;
        if let Progress::Unreadable(cause) = &measured {
            record["compile_progress"] = serde_json::json!({ "unreadable": cause });
        }
        if let Progress::Compiled {
            compiled,
            mut progress,
            previous,
        } = measured
        {
            if let Some(total) = previous {
                progress.insert("of_previous_run".into(), Value::from(total));
                // The measured ratio, as it is: a run that compiles more
                // crates than the last one reads past a hundred.
                // https://www.bipm.org/en/publications/si-brochure — percent
                // is the hundredth, the factor that turns a ratio into one.
                if let Some(ratio) = (compiled * 100).checked_div(total) {
                    progress.insert("percent".into(), Value::from(ratio));
                }
            }
            record["compile_progress"] = Value::Object(progress);
        }
    }
}
