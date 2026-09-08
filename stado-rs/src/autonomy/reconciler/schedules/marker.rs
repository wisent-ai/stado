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
    let fresh = value
        .get("started_at")
        .and_then(Value::as_str)
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .is_some_and(|started| {
            now.signed_duration_since(started.with_timezone(&Utc))
                .num_seconds()
                < policy.limits.decision_ttl_seconds as i64
        });
    if fresh {
        return Ok(false);
    }
    store
        .compare_and_swap_text(path, &existing.version, &content)
        .await?;
    Ok(true)
}
