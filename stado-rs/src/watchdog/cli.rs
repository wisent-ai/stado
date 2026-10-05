//! The watchdog schedule `stado serve --watchdog` runs.

use std::time::Duration;

use super::upload::once;
use super::DEFAULT_BUCKET;

/// Where diagnostics go, and how often they are collected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedArgs {
    pub bucket: String,
    /// Seconds between collections when the watchdog loops; required
    /// without `once`.
    pub interval_s: Option<i64>,
    pub once: bool,
}

pub(crate) fn configured_bucket() -> String {
    let bucket_env = crate::capabilities::config_env(
        crate::capabilities::RuntimeFacet::Storage,
        crate::capabilities::StorageAdapter::Gcs.id(),
        "bucket",
    )
    .expect("GCS bucket binding is missing from the capability catalog");
    std::env::var(bucket_env).unwrap_or_else(|_| DEFAULT_BUCKET.to_string())
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
