//! Selected-store SSH channel materialization.

use crate::deploy::ssh_key::{self, KeyFile};

/// Build one SSH invocation using only the target key in the credential store.
pub async fn channel_argv(
    target: &str,
    destination: &str,
    command: &str,
) -> Result<(Vec<String>, KeyFile), String> {
    let key = ssh_key::materialize(target)
        .await
        .map_err(|error| error.to_string())?;
    let argv = ssh_key::add_identity(
        vec![
            "ssh".to_string(),
            "-o".to_string(),
            "StrictHostKeyChecking=accept-new".to_string(),
            destination.to_string(),
            command.to_string(),
        ],
        &key,
    )
    .map_err(|error| error.to_string())?;
    Ok((argv, key))
}
