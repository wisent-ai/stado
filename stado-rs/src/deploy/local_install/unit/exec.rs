//! The argument vector a kind ExecStarts, resolved against the release
//! binaries [`crate::deploy::local_install::artifact`] places.

use crate::deploy::local_install::artifact::Bins;
use crate::deploy::DeployError;

/// `pub(super)` for [`super::env::build_env`], which asks the same question of
/// the same configuration to decide whether a coordinator unit carries the
/// agent's PATH.
pub(super) fn local_control_plane_configured() -> bool {
    crate::capabilities::storage_adapter(crate::config::wc_storage_backend())
        == Some(crate::capabilities::StorageAdapter::Local)
        && crate::config::wc_providers()
            .iter()
            .all(|provider| crate::capabilities::ProviderId::Local.matches(provider))
}

/// Python `_exec_args_for(entry, kind)`.
pub fn exec_args_for(bins: &Bins, kind: &str, name: &str) -> Result<Vec<String>, DeployError> {
    match kind {
        "host" => Ok(vec![
            bins.stado.clone(),
            "serve".to_string(),
            "--target".to_string(),
            name.to_string(),
        ]),
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
        // A failure-fixer unit is only ever read, never installed: the host
        // process folds a captured one in, and the captured file supplies its
        // whole argv and its cadence (`--failure-fixer-interval-seconds` from
        // its `sleep`). Only the program is compared against the capture.
        "failure-fixer" => Ok(vec!["/bin/bash".to_string()]),
        "watchdog" => Ok(vec![bins.stado_watchdog.clone()]),
        other => Err(DeployError(format!("unknown install kind: {other}"))),
    }
}
