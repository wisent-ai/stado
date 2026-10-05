//! The argument vector a kind ExecStarts, resolved against the release
//! binaries [`crate::deploy::local_install::artifact`] places.

use crate::deploy::local_install::artifact::Bins;
use crate::deploy::DeployError;

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
        // A failure-fixer unit is only ever read, never installed: the host
        // process folds a captured one in, and the captured file supplies its
        // whole argv and its cadence (`--failure-fixer-interval-seconds` from
        // its `sleep`). Only the program is compared against the capture.
        "failure-fixer" => Ok(vec!["/bin/bash".to_string()]),
        "watchdog" => Ok(vec![bins.stado_watchdog.clone()]),
        other => Err(DeployError(format!("unknown install kind: {other}"))),
    }
}
