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
        other => Err(DeployError(format!("unknown install kind: {other}"))),
    }
}
