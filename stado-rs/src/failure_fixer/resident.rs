//! The failure fixer as a role of `stado serve --failure-fixer-interval-seconds`:
//! each pass scans the failures that landed and dispatches one session for
//! every failure not dispatched yet.

use serde_json::json;

use crate::config;
use crate::queue::JobStorage;

use super::{scan_and_dispatch, FixError};

/// The host owns the scan loop; each pass resolves its storage and prints the
/// per-job results as one JSON document.
pub(crate) async fn run_resident(
    interval: std::num::NonZeroU64,
    command_pattern: Option<String>,
) -> Result<(), FixError> {
    let period = std::time::Duration::from_secs(interval.get());
    let mut schedule = tokio::time::interval(period);
    loop {
        schedule.tick().await;
        let result = async {
            let store = JobStorage::with_bucket(config::bucket()).await?;
            let results =
                scan_and_dispatch(None, command_pattern.as_deref(), true, &store, true).await?;
            let report = json!({"results": results, "count": results.len()});
            let pretty =
                serde_json::to_string_pretty(&report).expect("JSON serialization is infallible");
            println!("{}", crate::models::ensure_ascii(&pretty));
            Ok::<(), FixError>(())
        }
        .await;
        if let Err(error) = result {
            eprintln!("[stado serve failure-fixer] scan-and-dispatch failed: {error}");
        }
        // A pass that ran long is followed by a whole period, not by the
        // ticks it missed.
        schedule.reset();
    }
}
