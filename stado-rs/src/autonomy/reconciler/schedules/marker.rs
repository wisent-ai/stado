//! The once-per-occurrence marker that keeps two ticks, or two coordinators,
//! from acting on the same due schedule.

use chrono::{DateTime, Utc};
use serde_json::{json, Value};

use crate::queue::{JobStorage, StorageError};

use crate::autonomy::policy::AutonomyPolicy;

pub(super) async fn acquire_schedule_marker(
    store: &JobStorage,
    path: &str,
    policy: &AutonomyPolicy,
    now: DateTime<Utc>,
) -> Result<bool, StorageError> {
    let content = serde_json::to_string(&json!({
        "status": "in_progress",
        "started_at": now.to_rfc3339(),
    }))?;
    if store.create_text_if_absent(path, &content).await? {
        return Ok(true);
    }
    let Some(existing) = store.read_text_versioned(path).await? else {
        return Ok(false);
    };
    let value: Value = serde_json::from_str(&existing.content)?;
    if value.get("status").and_then(Value::as_str) == Some("completed") {
        return Ok(false);
    }
    // A marker is fresh while its start is inside the decision TTL. One
    // without a start, or with a start that is not RFC 3339, is not read as
    // stale and taken over: a second tick acting on it would run the
    // schedule twice, so it is refused with what the marker holds.
    let started_at = match value.get("started_at").and_then(Value::as_str) {
        Some(raw) => raw,
        None => {
            return Err(StorageError::Other(format!(
                "schedule marker {path} is neither completed nor carries a started_at: {value}"
            )))
        }
    };
    let started = DateTime::parse_from_rfc3339(started_at).map_err(|error| {
        StorageError::Other(format!(
            "schedule marker {path} carries started_at {started_at:?}, which is not RFC 3339: {error}"
        ))
    })?;
    let age_seconds = now
        .signed_duration_since(started.with_timezone(&Utc))
        .num_seconds();
    let fresh = i128::from(age_seconds) < i128::from(policy.limits.decision_ttl_seconds);
    if fresh {
        return Ok(false);
    }
    store
        .compare_and_swap_text(path, &existing.version, &content)
        .await?;
    Ok(true)
}
