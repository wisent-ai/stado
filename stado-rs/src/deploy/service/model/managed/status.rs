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
    /// five-day-old beacon is visibly five days old.
    pub reported_at: String,
    /// Why the state is what it is, when that is not self-evident.
    pub detail: String,
    /// Set when this unit's declared launchd domain is one its host cannot
    /// have. Carried on the row rather than recomputed by each surface,
    /// because the check needs the target's `role` and only the join has it.
    pub misdeclared_domain: Option<MisdeclaredDomain>,
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
        if let Some(misdeclared) = &self.misdeclared_domain {
            report.insert("misdeclared_domain".to_string(), misdeclared.to_json());
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
fn beacon_state(beacon: Option<&Map<String, Value>>, unit_id: &str) -> (String, String) {
    let Some(beacon) = beacon else {
        return (
            STATE_UNKNOWN.to_string(),
            "host has published no health beacon".to_string(),
        );
    };
    let units = beacon.get("units").and_then(Value::as_object);
    let Some(entry) = units.and_then(|units| units.get(unit_id)) else {
        return (
            STATE_MISSING.to_string(),
            "declared here; the latest beacon does not report it".to_string(),
        );
    };
    // The beacon writer emits {"state": ..., "detail": ...} per unit; older
    // beacons wrote a bare string. Both shapes are in flight, so read both.
    // The detail is why the state is what it is — for an `unreadable` unit
    // it is the only thing that says whether the host refused the read or
    // the read itself failed, and dropping it here would leave the operator
    // with a word and no cause.
    let (state, detail) = match entry {
        Value::String(state) => (state.clone(), String::new()),
        Value::Object(fields) => (
            fields
                .get("state")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            fields
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
        ),
        _ => (String::new(), String::new()),
    };
    if state.is_empty() {
        return (
            STATE_UNKNOWN.to_string(),
            "beacon reports the unit with no state".to_string(),
        );
    }
    (state, detail)
}

/// A beacon older than the fleet's one silence threshold cannot describe the
/// present. Callers still receive its timestamp and the reason it was refused,
/// but never a confident `active` or `missing` derived from stale evidence.
fn stale_beacon_detail(reported_at: &str, now: DateTime<Utc>) -> Option<String> {
    let threshold = crate::monitor::host_silence::silence_threshold_seconds();
    let Some(stamp) = DateTime::parse_from_rfc3339(reported_at)
        .ok()
        .map(|stamp| stamp.with_timezone(&Utc))
    else {
        return Some("health beacon has no usable reported_at; unit state is unknown".to_string());
    };
    let age = now.signed_duration_since(stamp).num_seconds();
    if age < i64::default() || age <= threshold {
        return None;
    }
    Some(format!(
        "health beacon is {age}s old, past the {threshold}s silence threshold; unit state is unknown"
    ))
}

/// Every registry-managed service on every kind=local host, with the state
/// the latest beacons report.
///
/// Beacons only: no ssh, no per-host round trip, so this stays answerable
/// while a host is wedged. A host that has never published a beacon yields
/// [`STATE_UNKNOWN`] rows instead of an error, because one silent host must
/// not blank the fleet-wide answer.
pub async fn list_services(store: &JobStorage) -> Result<Vec<ServiceStatus>, DeployError> {
    let registry = crate::deploy::host_channel::canonical_registry().await?;
    let mut rows: Vec<ServiceStatus> = Vec::new();
    for target in registry.local_targets() {
        let declared = declared_services(target);
        if declared.is_empty() {
            continue;
        }
        let report = match host_health::load_host_health(store, &target.name).await {
            Ok(report) => Some(report),
            Err(HostHealthError::NoBeacon { .. }) => None,
            Err(exc) => return Err(DeployError(exc.to_string())),
        };
        let beacon = report.as_ref().map(|report| &report.beacon);
        let reported_at = beacon
            .and_then(|beacon| beacon.get("reported_at"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();
        let stale = report
            .as_ref()
            .and_then(|_| stale_beacon_detail(&reported_at, Utc::now()));
        for service in declared {
            let (mut state, mut detail) = beacon_state(beacon, service.unit_id());
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
                misdeclared_domain,
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
    let mut rows = list_services(store).await?;
    rows.retain(|row| row.service.matches(name));
    Ok(rows)
}
