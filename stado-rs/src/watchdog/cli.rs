//! The watchdog schedule `stado serve --watchdog` runs.

use std::time::Duration;

use super::upload::once;

/// Where diagnostics go, and how often they are collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    pub bucket: String,
    /// Seconds between collections when the watchdog loops; required
    /// without `once`.
    pub interval_s: Option<i64>,
    pub once: bool,
}

/// The declared queue bucket (`crate::config::bucket`, its `WC_BUCKET`
/// binding or `config.json`). No bucket is built in: one used to be, so a
/// deployment that declared none uploaded its diagnostics to a bucket that
/// was not its own. An undeclared bucket is refused by name.
pub(crate) fn configured_bucket() -> Result<String, String> {
    match crate::config::bucket() {
        "" => Err(
            "the watchdog has no bucket: pass --watchdog-bucket or declare WC_BUCKET".to_string(),
        ),
        bucket => Ok(bucket.to_string()),
    }
}

/// Run the declared diagnostics schedule in the caller's process: one
/// collection per declared period, a collection that outlasts the period
/// starting the next at once and skipping the ones it missed. It returns only
/// when it cannot run, with the reason.
pub(crate) async fn run(parsed: &ParsedArgs) -> String {
    if parsed.once {
        let code = once(&parsed.bucket).await;
        return format!("one collection ran and returned exit status {code}");
    }
    let interval = match parsed.interval_s.map(u64::try_from) {
        Some(Ok(interval)) => std::num::NonZeroU64::new(interval),
        Some(Err(negative)) => {
            return format!("--watchdog-interval-seconds must not be negative: {negative}")
        }
        None => return "the collection loop needs --watchdog-interval-seconds".to_string(),
    };
    // A period of none ran collections back to back, uploading as fast as the
    // bucket answered.
    let Some(interval) = interval else {
        return "--watchdog-interval-seconds must be a positive number of seconds".to_string();
    };
    let mut schedule = tokio::time::interval(Duration::from_secs(interval.get()));
    schedule.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        schedule.tick().await;
        once(&parsed.bucket).await;
    }
}
