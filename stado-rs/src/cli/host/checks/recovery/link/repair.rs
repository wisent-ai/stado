use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::checks::health::api::host_health_beacon_unit;
use crate::cli::host::checks::health::beacon_store;
use crate::cli::host::checks::health::units::{collect_unit_log, host_health_publisher_diagnosis};
use crate::cli::host::checks::recovery::verifier::apply_object_verifier_repair;
use crate::cli::host::checks::{
    HOST_HEALTH_LOG_LINES, LINK_REPAIR_POLL_SECONDS, LINK_REPAIR_WAIT_SECONDS, NEWEST_SILENCES,
    OBJECT_API_SERVICE,
};

/// Apply the declared link repair and return the proof report to the repair
/// capability, which owns rendering.
pub(crate) async fn apply_link_repair(target: &str) -> Result<Value, CmdError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let resolved = crate::deploy::host_channel::resolve_target(&registry, target)
        .map_err(|error| CmdError::click(error.to_string()))?
        .clone();
    let store = beacon_store().await?;
    let initial_health = crate::monitor::host_health::load_host_health(&store, &resolved.name)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let initial_signal =
        crate::deploy::host_state::ping::grade_beacon(&initial_health, chrono::Utc::now());
    let threshold = crate::monitor::host_silence::silence_threshold_seconds();
    if initial_signal
        .age_seconds
        .is_some_and(|age| age <= threshold)
    {
        let report = json!({
            "target": resolved.name,
            "state": "already_healthy",
            "detail": "The newest beacon is inside the fleet silence threshold; no repair changed the verifier.",
            "beacon_age_seconds": initial_signal.age_seconds,
            "beacon_reported_at": initial_signal.reported_at,
        });
        return Ok(report);
    }

    let runner = crate::deploy::production_runner();
    let publisher_log = collect_unit_log(
        &resolved,
        host_health_beacon_unit(&resolved),
        HOST_HEALTH_LOG_LINES,
        &runner,
    )
    .await?;
    let diagnosis = host_health_publisher_diagnosis(&publisher_log);
    let diagnosis_code = diagnosis
        .get("code")
        .and_then(Value::as_str)
        .unwrap_or("unknown");
    if diagnosis_code != "verifier_unavailable" {
        let detail = diagnosis
            .get("detail")
            .and_then(Value::as_str)
            .unwrap_or("the publisher log names no supported repair");
        return Err(CmdError::click(format!(
            "{}: automatic link repair refused because the diagnosed publisher state is \
             {diagnosis_code}: {detail}",
            resolved.name
        )));
    }

    let authority = registry
        .service(OBJECT_API_SERVICE)
        .ok_or_else(|| {
            CmdError::click(format!(
                "service directory declares no {OBJECT_API_SERVICE}; refusing to guess which \
                 host owns host-health authorization"
            ))
        })?
        .active_host
        .clone();
    crate::deploy::host_channel::resolve_target(&registry, &authority)
        .map_err(|error| CmdError::click(error.to_string()))?;
    let verifier = apply_object_verifier_repair(&authority).await?;

    let previous_reported_at = initial_signal.reported_at.clone();
    let started = std::time::Instant::now();
    let mut last_observation = format!("beacon remained {:?}s old", initial_signal.age_seconds);
    loop {
        tokio::time::sleep(std::time::Duration::from_secs(LINK_REPAIR_POLL_SECONDS)).await;
        match crate::monitor::host_health::load_host_health(&store, &resolved.name).await {
            Ok(health) => {
                let signal =
                    crate::deploy::host_state::ping::grade_beacon(&health, chrono::Utc::now());
                last_observation = format!(
                    "newest beacon is {:?}s old and was reported at {}",
                    signal.age_seconds,
                    signal.reported_at.as_deref().unwrap_or("an unknown time")
                );
                let newer = signal.reported_at != previous_reported_at;
                let fresh = signal.age_seconds.is_some_and(|age| age <= threshold);
                if newer && fresh {
                    let newest_beacon_at = signal
                        .reported_at
                        .as_deref()
                        .and_then(crate::deploy::host_state::ping::parse_timestamp);
                    crate::monitor::host_silence::observe_beacon_age(
                        &store,
                        &resolved.name,
                        newest_beacon_at,
                        crate::monitor::host_silence::READER_CLI,
                        None,
                    )
                    .await
                    .map_err(|error| CmdError::click(error.to_string()))?;
                    let silences = crate::monitor::host_silence::recent_silences(
                        &store,
                        &resolved.name,
                        NEWEST_SILENCES,
                    )
                    .await
                    .map_err(|error| CmdError::click(error.to_string()))?;
                    let silence_closed = silences.iter().all(|record| record.ended_at.is_some());
                    let report = json!({
                        "target": resolved.name,
                        "state": "repaired",
                        "detail": if silence_closed {
                            "The verifier was reconciled, the host published a fresh beacon, and its open silence is closed."
                        } else {
                            "The verifier was reconciled and the host published a fresh beacon."
                        },
                        "authority": authority,
                        "diagnosis": diagnosis,
                        "verifier": verifier,
                        "previous_beacon_reported_at": previous_reported_at,
                        "beacon_reported_at": signal.reported_at,
                        "beacon_age_seconds": signal.age_seconds,
                        "silence_closed": silence_closed,
                        "waited_seconds": started.elapsed().as_secs(),
                    });
                    return Ok(report);
                }
            }
            Err(error) => {
                last_observation = error.to_string();
            }
        }
        if started.elapsed().as_secs() >= LINK_REPAIR_WAIT_SECONDS {
            return Err(CmdError::click(format!(
                "{}: reconciled the dashboard verifier on {authority}, but no fresh beacon \
                 arrived within {LINK_REPAIR_WAIT_SECONDS}s; {last_observation}",
                resolved.name
            )));
        }
    }
}
