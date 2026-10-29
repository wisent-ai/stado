//! Selected-store SSH channel materialization.

use crate::cli::CmdError;
use crate::deploy::host_access::ssh_key::{self, KeyFile};

/// Build one SSH invocation using only the target key in the credential store.
/// A key that cannot be materialized or attached keeps the class the deploy
/// layer stated for it.
pub async fn channel_argv(
    target: &str,
    destination: &str,
    command: &str,
) -> Result<(Vec<String>, KeyFile), CmdError> {
    let key = ssh_key::materialize(target).await?;
    let mut argv = vec![
        "ssh".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
    ];
    argv.extend(crate::deploy::host_access::ssh_destination_args(
        destination,
    ));
    argv.push(command.to_string());
    let argv = ssh_key::add_identity(argv, &key)?;
    Ok((argv, key))
}
