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

/// Run the declared diagnostics schedule in the caller's process. It returns
/// only when it cannot run, with the reason.
pub(crate) async fn run(parsed: &ParsedArgs) -> String {
    if parsed.once {
        let code = once(&parsed.bucket).await;
        return format!("one collection ran and returned exit status {code}");
    }
    let interval = match parsed.interval_s.map(u64::try_from) {
        Some(Ok(interval)) => interval,
        Some(Err(_)) => return "--watchdog-interval-seconds must not be negative".to_string(),
        None => return "the collection loop needs --watchdog-interval-seconds".to_string(),
    };
    loop {
        once(&parsed.bucket).await;
        tokio::time::sleep(Duration::from_secs(interval)).await;
    }
}
