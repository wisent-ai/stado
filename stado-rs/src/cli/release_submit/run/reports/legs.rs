//! What a listed run's platform legs say about the run: whether a required
//! one failed, and whether every submitted one is published.

use serde_json::Value;

use crate::queue::storage::JobStorage;

use super::jobs::platform_required;

/// Whether any failed leg of `run` is one the build manifest marks required.
/// Each failed leg is labelled `required` true or false, or carries
/// `required_unknown` with the reason the manifest could not be read; the
/// first required failure becomes the run's `failure` when it has none.
pub(super) async fn required_leg_failed(store: &JobStorage, run: &mut Value) -> bool {
    let Some(platforms) = run["platforms"].as_object() else {
        return false;
    };
    let failed: Vec<String> = platforms
        .iter()
        .filter(|(_, record)| record["state"].as_str() == Some("failed"))
        .map(|(name, _)| name.clone())
        .collect();
    let mut any = false;
    for platform in failed {
        match platform_required(store, run, &platform).await {
            Ok(required) => {
                run["platforms"][&platform]["required"] = Value::Bool(required);
                if required {
                    any = true;
                    if run["failure"].is_null() {
                        let text = match run["platforms"][&platform]["failure"].as_str() {
                            Some(text) => format!("{platform}: {text}"),
                            None => format!("{platform}: failed with no recorded reason"),
                        };
                        run["failure"] = Value::String(text);
                    }
                }
            }
            Err(why) => run["platforms"][&platform]["required_unknown"] = Value::String(why),
        }
    }
    any
}

/// Whether every platform of `run` that was submitted (carries a job) is
/// published, and at least one was.
pub(super) fn published_every_submitted_platform(run: &Value) -> bool {
    let Some(platforms) = run["platforms"].as_object() else {
        return false;
    };
    let submitted: Vec<&Value> = platforms
        .values()
        .filter(|record| record["job_id"].as_str().is_some_and(|job| !job.is_empty()))
        .collect();
    !submitted.is_empty()
        && submitted
            .iter()
            .all(|record| record["state"].as_str() == Some("published"))
}
