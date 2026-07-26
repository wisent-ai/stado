//! Remove a local account from a registry-managed host.
//!
//! `host user create` had no counterpart, so an account provisioned for a
//! one-off task could only be removed by hand over ad-hoc SSH — which is how
//! a test account outlived its purpose on a managed mac. Deletion runs through
//! the same approved channel, validates the name with the same rule, and
//! refuses the accounts a host cannot survive losing.
//!
//! The home directory goes with the account unless `keep_home` is set; the
//! remote script reports which of the two it did.

use std::time::Duration;

use crate::deploy::host_users::{
    ssh_argv, validate_ssh_target, validate_username, SSH_TIMEOUT_SECONDS,
};
use crate::deploy::{shlex_quote, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Marker prefix of the remote script's report line.
pub const STATUS_PREFIX: &str = "STADO_USER_DELETE\t";

/// Accounts that are never deletable through this path: the agent's own
/// service accounts and the operator logins a managed host depends on.
pub const PROTECTED_USERNAMES: &[&str] = &["root", "daemon", "nobody", "charles", "lukaszbartoszcze"];

/// Delete `$STADO_DELETE_USER`, honouring `$STADO_KEEP_HOME`.
pub const REMOTE_DELETE_SCRIPT: &str = r#"set -eu
username="${STADO_DELETE_USER:-}"
keep_home="${STADO_KEEP_HOME:-}"
os_name=$(/usr/bin/uname -s)

if [ -z "$username" ]; then
  printf 'STADO_USER_DELETE\tinvalid\t%s\t%s\n' "$os_name" "(empty)"
  exit
fi

if [ "$os_name" = "Darwin" ]; then
  if ! /usr/bin/dscl . -read "/Users/$username" >/dev/null 2>&1; then
    printf 'STADO_USER_DELETE\tabsent\t%s\t%s\n' "$os_name" "$username"
    exit
  fi
  if [ -n "$keep_home" ]; then
    /usr/sbin/sysadminctl -deleteUser "$username" -keepHome
    printf 'STADO_USER_DELETE\tdeleted-kept-home\t%s\t%s\n' "$os_name" "$username"
  else
    /usr/sbin/sysadminctl -deleteUser "$username"
    printf 'STADO_USER_DELETE\tdeleted\t%s\t%s\n' "$os_name" "$username"
  fi
else
  if ! /usr/bin/id "$username" >/dev/null 2>&1; then
    printf 'STADO_USER_DELETE\tabsent\t%s\t%s\n' "$os_name" "$username"
    exit
  fi
  if [ -n "$keep_home" ]; then
    /usr/sbin/userdel "$username"
    printf 'STADO_USER_DELETE\tdeleted-kept-home\t%s\t%s\n' "$os_name" "$username"
  else
    /usr/sbin/userdel -r "$username"
    printf 'STADO_USER_DELETE\tdeleted\t%s\t%s\n' "$os_name" "$username"
  fi
fi
"#;

/// One host's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteResult {
    pub target: String,
    pub ssh_target: String,
    pub status: String,
    pub os_name: String,
    pub error: Option<String>,
}

/// Reject the accounts that must not be removable through this command.
pub fn validate_deletable(username: &str) -> Result<(), DeployError> {
    validate_username(username)?;
    if PROTECTED_USERNAMES.contains(&username) {
        return Err(DeployError(format!(
            "refusing to delete protected account: {username}"
        )));
    }
    Ok(())
}

/// The privilege-escalating wrapper, matching account creation.
pub fn remote_command(username: &str, keep_home: bool) -> String {
    let keep = if keep_home { "keep" } else { "" };
    let invocation = format!(
        "/usr/bin/env STADO_DELETE_USER={} STADO_KEEP_HOME={} /bin/sh -c {}",
        shlex_quote(username),
        shlex_quote(keep),
        shlex_quote(REMOTE_DELETE_SCRIPT)
    );
    format!(
        "if [ \"$(/usr/bin/id -u)\" -eq 0 ]; then exec {invocation}; else exec /usr/bin/sudo -n {invocation}; fi"
    )
}

/// The last valid marker line wins, as in account creation.
pub fn parse_status(stdout: &str, username: &str) -> Result<(String, String), DeployError> {
    for line in stdout.lines().rev() {
        let Some(rest) = line.strip_prefix(STATUS_PREFIX) else { continue };
        let fields: Vec<&str> = rest.split('\t').collect();
        let Some(status) = fields.first() else { continue };
        let Some(os_name) = fields.get(usize::from(true)) else { continue };
        let Some(reported) = fields.get(usize::from(true) + usize::from(true)) else { continue };
        if *reported == username {
            return Ok((status.to_string(), os_name.to_string()));
        }
    }
    Err(DeployError(
        "remote host did not return a valid deletion status marker".to_string(),
    ))
}

/// Delete the account on one registry host.
pub async fn delete_user(
    username: &str,
    target: &ComputeTarget,
    keep_home: bool,
    runner: &Runner,
) -> DeleteResult {
    let mut result = DeleteResult {
        target: target.name.clone(),
        ssh_target: String::new(),
        status: String::new(),
        os_name: String::new(),
        error: None,
    };
    if let Err(error) = validate_deletable(username) {
        result.error = Some(error.0);
        return result;
    }
    let destination = target.ssh.as_deref().unwrap_or("");
    if destination.is_empty() {
        result.error = Some(format!(
            "target {} has no ssh destination in the registry",
            target.name
        ));
        return result;
    }
    if let Err(error) = validate_ssh_target(destination) {
        result.error = Some(error.0);
        return result;
    }
    result.ssh_target = destination.to_string();

    let mut spec = CommandSpec::new(ssh_argv(destination, &remote_command(username, keep_home)));
    spec.timeout = Some(Duration::from_secs(SSH_TIMEOUT_SECONDS));
    match runner(spec).await {
        Ok(output) if output.ok() => match parse_status(&output.stdout, username) {
            Ok((status, os_name)) => {
                result.status = status;
                result.os_name = os_name;
            }
            Err(error) => result.error = Some(error.0),
        },
        Ok(output) => result.error = Some(output.detail().trim().to_string()),
        Err(error) => result.error = Some(error),
    }
    result
}
