use std::sync::Arc;

use serde::Deserialize;

use crate::deploy::service::*;

// ---------------------------------------------------------------------------
// Read side: the beacon join
// ---------------------------------------------------------------------------

/// A managed unit with the state the latest beacon reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceStatus {
    pub service: ManagedService,
    /// One of [`STATE_ACTIVE`], [`STATE_INACTIVE`], [`STATE_FAILED`],
    /// [`STATE_MISSING`], [`STATE_UNKNOWN`] — or whatever other word the
    /// beacon used, passed through verbatim rather than flattened into
    /// "unknown".
    pub state: String,
    /// The beacon's `reported_at`, so a confident-looking `active` from a
    /// five-day-old beacon is visibly five days old. `None` when the host
    /// published no beacon or its beacon names no report time; JSON `null`.
    pub reported_at: Option<String>,
    /// Why the state is what it is, when that is not self-evident.
    pub detail: String,
    /// The host's API-question reading for this unit, absent when no
    /// question is declared or no beacon carried one.
    pub answering: Option<Value>,
    /// Why a present API-question reading cannot be interpreted, preserving
    /// the raw value in `answering` for CLI and GUI diagnostics.
    pub answering_error: Option<String>,
    /// Set when this unit's declared launchd domain is one its host cannot
    /// have. Carried on the row rather than recomputed by each surface,
    /// because the check needs the target's `role` and only the join has it.
    pub misdeclared_domain: Option<MisdeclaredDomain>,
    /// The host's role register from its beacon: `held` (role, pid,
    /// program, since) and `ended_strays`; none on a beacon older than the
    /// register.
    pub roles: Option<Arc<Value>>,
}

impl ServiceStatus {
    pub fn to_json(&self) -> Value {
        let mut report = match self.service.to_json() {
            Value::Object(map) => map,
            other => return other,
        };
        report.insert("state".to_string(), json!(self.state));
        report.insert("reported_at".to_string(), json!(self.reported_at));
        report.insert("detail".to_string(), json!(self.detail));
        if let Some(answering) = &self.answering {
            report.insert("answering".to_string(), answering.clone());
        }
        if let Some(error) = &self.answering_error {
            report.insert("answering_error".to_string(), json!(error));
        }
        if let Some(misdeclared) = &self.misdeclared_domain {
            report.insert("misdeclared_domain".to_string(), misdeclared.to_json());
        }
        if let Some(roles) = &self.roles {
            report.insert("roles".to_string(), roles.as_ref().clone());
        }
        Value::Object(report)
    }
}

/// Resolve one unit's state out of a host beacon.
///
/// `beacon` is `None` when the host has published nothing at all, which is
/// a different fact from "the beacon does not carry this unit" and is kept
/// as a different state: conflating a silent host with a missing unit is
/// the class of mistake this whole module exists to stop.
fn beacon_state(
    beacon: Option<&Map<String, Value>>,
    unit_id: &str,
) -> (String, String, Option<Value>) {
    let Some(beacon) = beacon else {
        return (
            STATE_UNKNOWN.to_string(),
            "host has published no health beacon".to_string(),
            None,
        );
    };
    let units = match beacon.get("units") {
        Some(Value::Object(units)) => units,
        Some(value) => {
            return (
                STATE_UNKNOWN.to_string(),
                format!("health beacon units is not an object: {value}"),
                None,
            )
        }
        None => {
            return (
                STATE_UNKNOWN.to_string(),
                "health beacon has no units map".to_string(),
                None,
            )
        }
    };
    let Some(entry) = units.get(unit_id) else {
        return (
            STATE_MISSING.to_string(),
            "declared here; the latest beacon does not report it".to_string(),
            None,
        );
    };
    let answering = entry.get("answering").cloned();
    // Older beacons wrote a bare state string; a structured unit may carry
    // the host's actual error and its independent API-question reading.
    let (state, detail) = match entry {
        Value::String(state) => (state.clone(), String::new()),
        Value::Object(fields) => {
            let state = match fields.get("state") {
                Some(Value::String(state)) => state.clone(),
                Some(value) => {
                    return (
                        STATE_UNKNOWN.to_string(),
                        format!("beacon unit {unit_id} state is not a string: {value}"),
                        answering,
                    )
                }
                None => {
                    return (
                        STATE_UNKNOWN.to_string(),
                        format!("beacon unit {unit_id} has no state"),
                        answering,
                    )
                }
            };
            let detail = match fields.get("detail") {
                Some(Value::String(detail)) => detail.clone(),
                None => String::new(),
                Some(value) => {
                    return (
                        STATE_UNKNOWN.to_string(),
                        format!("beacon unit {unit_id} detail is not a string: {value}"),
                        answering,
                    )
                }
            };
            (state, detail)
        }
        other => {
            return (
                STATE_UNKNOWN.to_string(),
                format!("beacon unit {unit_id} is not a state or object: {other}"),
                answering,
            )
        }
    };
    if state.is_empty() {
        return (
            STATE_UNKNOWN.to_string(),
            format!("beacon unit {unit_id} has an empty state"),
            answering,
        );
    }
    (state, detail, answering)
}

/// A beacon whose own promised next publication has passed cannot describe
/// the present. Callers still receive its timestamp and the reason it was
/// refused, but never a confident `active` or `missing` derived from stale
/// evidence. The verdict is the one `host ping` reads.
fn stale_beacon_detail(
    report: &host_health::HostHealthReport,
    now: DateTime<Utc>,
) -> Option<String> {
    let signal = crate::deploy::host_state::ping::grade_beacon(report, now);
    if signal.verdict == crate::deploy::host_state::ping::Verdict::Ok {
        return None;
    }
    Some(match (signal.age_seconds, signal.next_by, signal.error) {
        (_, _, Some(error)) => format!("health beacon refused: {error}; unit state is unknown"),
        (Some(age), Some(next_by), None) => format!(
            "health beacon is {age}s old, past the next publication its host promised by \
             {next_by}; unit state is unknown"
        ),
        _ => "health beacon has no usable reported_at; unit state is unknown".to_string(),
    })
}

/// Every registry-managed service on every kind=local host, with the state
/// the latest beacons report.
///
/// Beacons only: no ssh, no per-host round trip, so this stays answerable
/// while a host is wedged. A host that has never published a beacon yields
/// [`STATE_UNKNOWN`] rows instead of an error, because one silent host must
/// not blank the fleet-wide answer.
pub async fn list_services(store: &JobStorage) -> Result<Vec<ServiceStatus>, DeployError> {
    read_services(store, None).await
}

async fn read_services(
    store: &JobStorage,
    name: Option<&str>,
) -> Result<Vec<ServiceStatus>, DeployError> {
    let registry = crate::deploy::host_channel::canonical_registry().await?;
    // Independent beacon reads start together; results retain registry order,
    // so a slow host cannot delay asking every other host for its last report.
    let reports = futures::future::try_join_all(
        registry
            .local_targets()
            .into_iter()
            .filter_map(|target| {
                let mut declared = declared_services(target);
                if let Some(name) = name {
                    declared.retain(|service| service.matches(name));
                }
                (!declared.is_empty()).then_some((target, declared))
            })
            .map(|(target, declared)| async move {
                let report = match host_health::load_host_health(store, &target.name).await {
                    Ok(report) => Some(report),
                    Err(HostHealthError::NoBeacon { .. }) => None,
                    Err(exc) => return Err(DeployError::from(crate::cli::CmdError::from(exc))),
                };
                Ok::<_, DeployError>((target, declared, report))
            }),
    )
    .await?;
    let mut rows: Vec<ServiceStatus> =
        Vec::with_capacity(reports.iter().map(|(_, declared, _)| declared.len()).sum());
    for (target, declared, report) in reports {
        let beacon = report.as_ref().map(|report| &report.beacon);
        let reported_at = beacon
            .and_then(|beacon| beacon.get("reported_at"))
            .and_then(Value::as_str)
            .map(str::to_string);
        let stale = report
            .as_ref()
            .and_then(|report| stale_beacon_detail(report, Utc::now()));
        // The host's role register, as the beacon's last pass published it:
        // which roles a process holds (pid, program, since) and the strays
        // the pass ended. Each unit row of the host carries it, so `service
        // list` says how many processes play a role there and from which
        // program, not `active` alone. A beacon from before the register
        // carries none.
        let roles = beacon
            .and_then(|beacon| beacon.get("roles"))
            .cloned()
            .map(Arc::new);
        for service in declared {
            let (mut state, mut detail, answering) = beacon_state(beacon, service.unit_id());
            let answering_error = answering.as_ref().and_then(|reading| {
                crate::deploy::service_answering::Reading::deserialize(reading)
                    .err()
                    .map(|cause| {
                        format!(
                            "beacon unit {} has malformed answering: {cause}; value: {reading}",
                            service.unit_id()
                        )
                    })
            });
            if let Some(stale) = &stale {
                state = STATE_UNKNOWN.to_string();
                detail = stale.clone();
            }
            let misdeclared_domain = MisdeclaredDomain::detect(target, &service);
            rows.push(ServiceStatus {
                service,
                state,
                reported_at: reported_at.clone(),
                detail,
                answering,
                answering_error,
                misdeclared_domain,
                roles: roles.clone(),
            });
        }
    }
    Ok(rows)
}

/// [`list_services`] narrowed to the units one NAME addresses. An empty
/// result is the caller's error to raise: "no managed service named X" and
/// "X is managed but reports nothing" are different answers.
pub async fn find_services(
    store: &JobStorage,
    name: &str,
) -> Result<Vec<ServiceStatus>, DeployError> {
    read_services(store, Some(name)).await
}
