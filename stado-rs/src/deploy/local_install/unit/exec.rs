//! The argument vector a kind ExecStarts, resolved against the release
//! binaries [`crate::deploy::local_install::artifact`] places.

use crate::deploy::local_install::artifact::Bins;
use crate::deploy::DeployError;

/// `pub(super)` for [`super::env::build_env`], which asks the same question of
/// the same configuration to decide whether a coordinator unit carries the
/// agent's PATH and WC_PYTHON.
pub(super) fn local_control_plane_configured() -> bool {
    crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
        == Some(crate::capabilities::StorageAdapter::Local)
        && crate::config::wc_providers()
            .iter()
            .all(|provider| crate::capabilities::ProviderId::Local.matches(provider))
}

/// Python `_exec_args_for(entry, kind)`.
pub fn exec_args_for(bins: &Bins, kind: &str, _name: &str) -> Result<Vec<String>, DeployError> {
    match kind {
        "agent" => Ok(vec![
            bins.stado.clone(),
            "agent".to_string(),
            "--auto".to_string(),
        ]),
        "coordinator" if local_control_plane_configured() => {
            Ok(vec![bins.stado.clone(), "local-control-plane".to_string()])
        }
        "coordinator" => Ok(vec![
            bins.stado.clone(),
            "cloud-control-plane".to_string(),
            "--bind".to_string(),
            crate::config::dashboard_bind().to_string(),
            "--port".to_string(),
            crate::config::dashboard_port().to_string(),
        ]),
        "disk-cleanup" => Ok(vec![
            bins.stado.clone(),
            "disk-cleanup".to_string(),
            "--watch".to_string(),
        ]),
        "failure-fixer" => {
            // Run scan_and_dispatch every iteration. Loop in shell so a
            // single failure of scan_and_dispatch (transient GCS hiccup,
            // model-router 5xx) does not require launchd to restart the
            // whole job; the next iteration retries cleanly.
            let pattern = crate::config::FAILURE_FIXER_COMMAND_PATTERN;
            let pat_arg = if pattern.is_empty() {
                String::new()
            } else {
                format!("--command-pattern '{pattern}'")
            };
            Ok(vec![
                "/bin/bash".to_string(),
                "-c".to_string(),
                format!(
                    "while true; do {} scan-dispatch --execute {pat_arg}; sleep {}; done",
                    bins.stado_fix,
                    crate::config::FAILURE_FIXER_TICK_SECONDS
                ),
            ])
        }
        "watchdog" => Ok(vec![bins.stado_watchdog.clone()]),
        other => Err(DeployError(format!("unknown install kind: {other}"))),
    }
}
