use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::checks::health::api::host_health_beacon_unit;
use crate::cli::host::checks::health::beacon_store;
use crate::cli::host::checks::health::units::{collect_unit_log, host_health_publisher_diagnosis};
use crate::cli::host::checks::recovery::verifier::apply_object_verifier_repair;
use crate::cli::host::checks::{HOST_HEALTH_LOG_LINES, OBJECT_API_SERVICE};

/// Apply the declared link repair and return the proof report to the repair
/// capability, which owns rendering.
pub(crate) async fn apply_link_repair(target: &str) -> Result<Value, CmdError> {
    let registry = crate::targets::fetch_registry_remote()
        .await
        .map_err(CmdError::from)?;
    let resolved = crate::cli::resolved_host(&registry, target)?.clone();
    let store = beacon_store().await?;
    let initial_health = crate::monitor::host_health::load_host_health(&store, &resolved.name)
        .await
        .map_err(CmdError::from)?;
    let initial_signal =
        crate::deploy::host_state::ping::grade_beacon(&initial_health, chrono::Utc::now());
    // The beacon's own promise decides, the same verdict `host ping` reads.
    if initial_signal.verdict == crate::deploy::host_state::ping::Verdict::Ok {
        let report = json!({
            "target": resolved.name,
            "state": "already_healthy",
            "detail": "The newest beacon is within the next publication its host promised; no repair changed the verifier.",
            "beacon_age_seconds": initial_signal.age_seconds,
            "beacon_next_by": initial_signal.next_by,
            "beacon_reported_at": initial_signal.reported_at,
        });
        return Ok(report);
    }

    let runner = crate::deploy::production_runner();
    let publisher_log = collect_unit_log(
        &resolved,
        &host_health_beacon_unit(&resolved)?,
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
        return Err(CmdError::refused(format!(
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
            .stating(crate::primitives::failure::FailureCode::Config)
        })?
        .active_host
        .clone();
    crate::cli::resolved_host(&registry, &authority)?;
    let verifier = apply_object_verifier_repair(&authority).await?;

    let previous_reported_at = initial_signal.reported_at.clone();
    let health = crate::monitor::host_health::load_host_health(&store, &resolved.name)
        .await
        .map_err(CmdError::from)?;
    let signal = crate::deploy::host_state::ping::grade_beacon(&health, chrono::Utc::now());
    let newer = signal.reported_at != previous_reported_at;
    let fresh = signal.verdict == crate::deploy::host_state::ping::Verdict::Ok;
    if !(newer && fresh) {
        return Err(CmdError::click(format!(
            "{}: reconciled the dashboard verifier on {authority}, and the host has not published \
             a fresh beacon since: the newest is {:?}s old and was reported at {}. The repair \
             is proven by the host's next beacon; read it with the same check.",
            resolved.name,
            signal.age_seconds,
            signal.reported_at.as_deref().unwrap_or("an unknown time")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    let newest_beacon_at = signal
        .reported_at
        .as_deref()
        .and_then(crate::deploy::host_state::ping::parse_timestamp);
    let next_by = signal
        .next_by
        .as_deref()
        .and_then(crate::deploy::host_state::ping::parse_timestamp);
    crate::monitor::host_silence::observe_beacon_age(
        &store,
        &resolved.name,
        newest_beacon_at,
        next_by,
        crate::monitor::host_silence::READER_CLI,
        None,
    )
    .await
    .map_err(CmdError::from)?;
    // Only the newest silence can be open; it is the one this repair closed.
    let silence_closed = crate::monitor::host_silence::open_silence(&store, &resolved.name)
        .await
        .map_err(CmdError::from)?
        .is_none();
    Ok(json!({
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
    }))
}
