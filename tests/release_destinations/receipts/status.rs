//! Restoring the worker's genuine receipt after each altered case, and the
//! check that the CLI, the native app and the text status all report the
//! delivery exactly as the run persisted it.

use super::super::runner::{require, Journey};
use super::upload;
use stado::release_pipeline::{DeliveryRun, DeliveryRunState, ReleaseRun, ReleaseRunState};
use std::path::PathBuf;

/// The worker's own receipt and the accepted delivery it proves, which every
/// altered case is undone back to.
pub(super) struct Genuine<'a> {
    pub(super) receipt_uri: &'a str,
    pub(super) bytes: &'a [u8],
    pub(super) job: &'a str,
    pub(super) accepted: &'a str,
    pub(super) run_uri: &'a str,
}

/// Put the genuine receipt bytes back and require that resuming the run
/// recovers the same accepted delivery, persisted exactly as answered.
pub(super) fn restore(
    journey: &mut Journey,
    run: &ReleaseRun,
    delivery: &str,
    genuine: &Genuine,
) -> Result<(), String> {
    upload(
        journey,
        genuine.receipt_uri,
        "restore-redelivery-receipt.json",
        genuine.bytes,
    )?;
    let completed = journey.cli(&["release", "resume", &run.run_id, "--json"], true)?;
    let completed: ReleaseRun =
        serde_json::from_value(completed).map_err(|error| error.to_string())?;
    let leg = completed
        .deliveries
        .get(delivery)
        .ok_or("restored delivery disappeared")?;
    require(
        completed.state == ReleaseRunState::Completed
            && leg.state == DeliveryRunState::Passed
            && leg.job_id == genuine.job
            && leg.receipt_sha256.as_deref() == Some(genuine.accepted)
            && leg.failure.is_none(),
        "restoring genuine receipt bytes did not recover the same accepted delivery",
    )?;
    let persisted = journey.cli(&["storage", "cat", genuine.run_uri], true)?;
    require(
        serde_json::to_value(completed).map_err(|error| error.to_string())? == persisted,
        "restored resume response differs from the persisted run",
    )
}

pub(super) async fn qualify_status(
    journey: &mut Journey,
    run: &ReleaseRun,
    delivery: &str,
    expected: &DeliveryRun,
) -> Result<(), String> {
    let arguments = ["release", "status", "--run", &run.run_id, "--json"];
    let cli = journey.cli(&arguments, true)?;
    let native = super::super::api::command(journey, &arguments, false, false).await?;
    for (surface, response) in [("CLI", cli), ("native", native)] {
        let runs = response["runs"]
            .as_array()
            .ok_or("status response has no runs")?;
        let observed = runs
            .iter()
            .find(|candidate| candidate["run_id"] == run.run_id)
            .ok_or_else(|| format!("{surface} status lost run {}", run.run_id))?;
        let actual: DeliveryRun = serde_json::from_value(observed["deliveries"][delivery].clone())
            .map_err(|cause| format!("{surface} delivery {delivery} did not decode: {cause}"))?;
        require(
            actual == *expected,
            &format!(
                "{surface} delivery status differs from persisted job, state or accepted proof"
            ),
        )?;
    }
    let binary = PathBuf::from(journey.command().get_program());
    let text = journey.execute(
        &binary,
        &["release", "status", "--run", &run.run_id],
        true,
        true,
    )?;
    require(
        !expected.output_prefix.is_empty(),
        "completed worker has no retained output prefix",
    )?;
    for value in [&expected.job_id, &expected.output_prefix] {
        require(
            text.contains(value),
            &format!("text status lost delivery evidence location {value}"),
        )?;
    }
    if let Some(digest) = &expected.receipt_sha256 {
        require(
            text.contains(digest),
            "text status lost the previously accepted receipt digest",
        )?;
    }
    if let Some(failure) = &expected.failure {
        for cause in failure.lines() {
            require(
                text.contains(cause),
                &format!("text status lost actual failure: {cause}"),
            )?;
        }
    }
    Ok(())
}
