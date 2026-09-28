//! The receipts: what one accepted capture is called afterwards, the four
//! states this command reports, and the refusals a batch id earns.

mod batch;
mod record;

use serde_json::{json, Value};

use super::channel::Channel;
use super::{Plan, CAPTURE_ACTION, REQUEST_DEADLINE, RUN_ROUTE};
use crate::deploy::DeployError;

pub use batch::{status, totals};

/// One accepted capture and the action id the worker will run it under.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enqueued {
    pub action_id: String,
    pub site_slug: String,
    pub axis: String,
    pub artifact_prefix: String,
}

/// Execute every capture through Weles's current synchronous `/run` surface.
///
/// The old admission API and its database queue were removed from Weles.
/// Returning only after each run finishes means an accepted row already has a
/// final run id and its artifacts have either been uploaded or the command has
/// failed with the worker's exact reason. After every run the batch's record
/// in Stado storage is rewritten, so a batch stopped halfway still reports the
/// runs it finished and the refusal that stopped it.
pub async fn enqueue(channel: &Channel, plan: &Plan) -> Result<Vec<Enqueued>, DeployError> {
    let mut accepted = Vec::with_capacity(plan.captures.len());
    let mut receipts = Vec::with_capacity(plan.captures.len());
    for capture in &plan.captures {
        let outcome = channel
            .call(
                RUN_ROUTE,
                &json!({
                    "action": CAPTURE_ACTION,
                    "params": Value::Object(capture.params.clone()),
                    "creds": "redact",
                    "timeout_ms": REQUEST_DEADLINE.as_millis(),
                }),
            )
            .await
            .and_then(|payload| {
                payload
                    .get("run_id")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(str::to_string)
                    .ok_or_else(|| {
                        DeployError(
                            "the Weles API completed the capture and returned no run id"
                                .to_string(),
                        )
                    })
            });
        let receipt = |run_id: String, state: &str, error: Option<String>| record::Receipt {
            run_id,
            site_slug: capture.site_slug.clone(),
            axis: capture.axis.clone(),
            artifact_prefix: capture.artifact_prefix.clone(),
            state: state.to_string(),
            error,
        };
        match outcome {
            Ok(run_id) => {
                receipts.push(receipt(run_id.clone(), STATE_DONE, None));
                record::write(&plan.batch, &receipts).await?;
                accepted.push(Enqueued {
                    action_id: run_id,
                    site_slug: capture.site_slug.clone(),
                    axis: capture.axis.clone(),
                    artifact_prefix: capture.artifact_prefix.clone(),
                });
            }
            Err(error) => {
                receipts.push(receipt(
                    String::new(),
                    STATE_FAILED,
                    Some(error.to_string()),
                ));
                record::write(&plan.batch, &receipts).await?;
                return Err(error);
            }
        }
    }
    Ok(accepted)
}

/// The two states a finished synchronous run is recorded as.
pub const STATE_DONE: &str = "done";
pub const STATE_FAILED: &str = "failed";

/// One capture as Stado's batch record and the object store describe it now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureState {
    pub action_id: String,
    pub site_slug: String,
    pub axis: String,
    pub state: String,
    pub error: Option<String>,
    pub artifact_prefix: String,
    /// Artifact and sidecar URIs already under this capture's prefix.
    pub artifacts: Vec<String>,
}

/// One batch as Stado's batch record and the object store describe it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BatchStatus {
    pub captures: Vec<CaptureState>,
    /// Why the artifact listing could not be made, when it could not.
    ///
    /// `None` means the store answered and every `artifacts` list is what is
    /// really there. `Some` means nobody could ask, and the empty lists are the
    /// absence of an answer rather than the absence of objects — the same
    /// distinction `stado storage ls` draws between an empty prefix and an
    /// unreachable one, and for the same reason: on this fleet those two states
    /// were indistinguishable through one method's return value, and a
    /// forbidden store read exactly like a drained one.
    pub artifacts_unreachable: Option<String>,
}
