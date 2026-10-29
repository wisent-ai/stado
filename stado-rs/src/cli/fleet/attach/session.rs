//! The SSH session settings an attached machine's resolver needs.
//!
//! An attached machine runs a resolver whose authority tunnel is a
//! long-lived SSH session, and Stado refuses such a session (and its
//! install gate refuses the release) until `ssh.keepalive_seconds` and
//! `ssh.keepalive_count_max` are declared. The machine is given the values
//! this host declares, the one the fleet already runs its own tunnel with.

use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;
use crate::targets::ComputeTarget;

/// The keepalive declarations, by configuration key and environment name.
const KEEPALIVE: &[(&str, &str)] = &[
    ("ssh.keepalive_seconds", "STADO_SSH_KEEPALIVE_SECONDS"),
    ("ssh.keepalive_count_max", "STADO_SSH_KEEPALIVE_COUNT_MAX"),
];

/// Declare on `target` the SSH keepalive this host declares.
pub(super) async fn declare_keepalive(
    target: &ComputeTarget,
    echo: &mut dyn FnMut(&str),
) -> Result<(), CmdError> {
    for (key, env) in KEEPALIVE {
        let value = crate::config_file::resolve(env, key, "");
        let value = value.trim();
        if value.is_empty() {
            return Err(CmdError::click(format!(
                "this host declares no {key} (env {env}) to give {}, whose resolver keeps a \
                 long-lived session; declare it here with `stado config set {key} VALUE`",
                target.name
            ))
            .stating(FailureCode::Config));
        }
        crate::cli::host::write_host_config(&target.name, key, value).await?;
        echo(&format!("[attach] {}: {key} = {value}, as this host declares it", target.name));
    }
    Ok(())
}
