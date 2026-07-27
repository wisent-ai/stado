//! `stado disk-cleanup` / `stado install-disk-cleanup`.
//!
//! Port of the `disk_cleanup` and `install_disk_cleanup` commands from
//! `stado/cli.py`. The install side is the `kind == "disk-cleanup"` slice
//! of `stado/deploy/local_install.py` and now delegates to the shared
//! implementation in [`crate::deploy::local_install`] — faithful to
//! `install_local(SimpleNamespace(name="disk-cleanup"), "disk-cleanup",
//! False, click.echo)`, including the guard that never puts the HF write
//! token into the disk-cleanup unit env.

use std::time::Duration;

use serde_json::Value;

use super::CmdError;
use crate::providers::local::disk_cleanup;

/// `disk-cleanup` command body (Python `disk_cleanup`).
pub async fn run(once: bool, watch: bool) -> Result<(), CmdError> {
    if once && watch {
        // click.UsageError: message on stderr, exit code 2.
        return Err(CmdError {
            message: Some("--once and --watch are mutually exclusive".to_string()),
            code: 2,
        });
    }
    loop {
        let report = disk_cleanup::run_cleanup_once(0, false, &mut |_message| {}).await;
        println!("{}", disk_cleanup::canonical_json(&report));
        if !watch {
            return Ok(());
        }
        let interval = report
            .get("check_interval_seconds")
            .and_then(Value::as_i64)
            .unwrap_or(60);
        tokio::time::sleep(Duration::from_secs(interval.max(60) as u64)).await;
    }
}

/// `install-disk-cleanup` command body (Python `install_disk_cleanup` →
/// `install_local(..., "disk-cleanup", False, click.echo)`; dry_run is
/// always false from this command).
pub async fn install() -> Result<(), CmdError> {
    let runner = crate::deploy::production_runner();
    let hf_fetch = crate::deploy::local_install::production_hf_fetcher();
    let mut echo = |line: &str| println!("{line}");
    crate::deploy::local_install::install_local(
        "disk-cleanup",
        "disk-cleanup",
        false,
        &runner,
        &hf_fetch,
        &mut echo,
    )
    .await
    .map_err(|exc| CmdError::click(exc.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn once_and_watch_are_mutually_exclusive() {
        let err = run(true, true).await.unwrap_err();
        assert_eq!(err.message.as_deref(), Some("--once and --watch are mutually exclusive"));
        assert_eq!(err.code, 2);
    }
}
