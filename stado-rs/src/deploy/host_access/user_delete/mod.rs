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

use crate::deploy::host_users::{validate_username, SSH_TIMEOUT_SECONDS};
use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Marker prefix of the remote script's report line.
pub const STATUS_PREFIX: &str = "STADO_USER_DELETE\t";

/// Accounts that are never deletable through this path.
///
/// The system accounts come from `protected-accounts.json` beside this
/// module. The operator logins do not: they used to be two names written
/// into this list, which protected exactly those two people and nobody
/// else on the fleet. The login a host is actually reached by is read from
/// that host's own registry route instead.
fn protected_system_accounts() -> Vec<String> {
    let declared: serde_json::Value = serde_json::from_str(include_str!("protected-accounts.json"))
        .expect("protected-accounts.json beside this module is valid JSON");
    declared["system_accounts"]
        .as_array()
        .expect("protected-accounts.json declares a system_accounts array")
        .iter()
        .filter_map(|name| name.as_str().map(str::to_owned))
        .collect()
}

/// The logins this target's registry routes authenticate as, so Stado
/// cannot delete the account it reaches the host with.
fn route_logins(target: &ComputeTarget) -> Vec<String> {
    target
        .ssh_connections()
        .filter_map(|(_, destination)| {
            destination
                .split_once('@')
                .map(|(login, _)| login.trim().to_owned())
        })
        .filter(|login| !login.is_empty())
        .collect()
}

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

/// Reject the accounts that must not be removable through this command:
/// the declared system accounts, and the login this host is reached by.
pub fn validate_deletable(
    username: &str,
    target: Option<&ComputeTarget>,
) -> Result<(), DeployError> {
    validate_username(username)?;
    if protected_system_accounts()
        .iter()
        .any(|account| account == username)
    {
        return Err(DeployError(format!(
            "refusing to delete protected system account: {username}"
        )));
    }
    if let Some(target) = target {
        if route_logins(target).iter().any(|login| login == username) {
            return Err(DeployError(format!(
                "refusing to delete {username}: it is the login the registry reaches {} with",
                target.name
            )));
        }
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
        let Some(rest) = line.strip_prefix(STATUS_PREFIX) else {
            continue;
        };
        let fields: Vec<&str> = rest.split('\t').collect();
        let Some(status) = fields.first() else {
            continue;
        };
        let Some(os_name) = fields.get(usize::from(true)) else {
            continue;
        };
        let Some(reported) = fields.get(usize::from(true) + usize::from(true)) else {
            continue;
        };
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
    if let Err(error) = validate_deletable(username, Some(target)) {
        result.error = Some(error.0);
        return result;
    }
    if !target.has_ssh_connection() {
        result.error = Some(format!(
            "target {} has no SSH connection path in the registry",
            target.name
        ));
        return result;
    }

    let execution = host_channel::run_script_with_timeout_and_connection(
        target,
        &remote_command(username, keep_home),
        Duration::from_secs(SSH_TIMEOUT_SECONDS),
        runner,
    )
    .await;
    let output = match execution {
        Ok((output, host_channel::UsedConnection::Ssh(connection))) => {
            result.ssh_target = connection.destination.to_string();
            Ok(output)
        }
        Ok((output, host_channel::UsedConnection::Local)) => Ok(output),
        Err(error) => Err(error.0),
    };
    match output {
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

#[cfg(test)]
mod tests {
    use super::validate_deletable;
    use crate::targets::ComputeTarget;
    /// A registry row shaped as the canonical document writes it.
    fn reached_as(login: &str) -> ComputeTarget {
        serde_json::from_value(serde_json::json!({
            "name": "some-host",
            "kind": "local",
            "ssh": format!("{login}@some-host.local"),
        }))
        .expect("a registry row with a name, a kind and a route")
    }

    #[test]
    fn a_declared_system_account_is_refused_on_any_host() {
        let error = validate_deletable("root", Some(&reached_as("operator")))
            .expect_err("root is protected");
        assert!(error.0.contains("protected system account"), "{}", error.0);
    }

    /// The login the registry reaches a host with is protected wherever it
    /// appears, which is what two operator names in a list could not do.
    #[test]
    fn the_login_this_host_is_reached_by_is_refused() {
        let error = validate_deletable("operator", Some(&reached_as("operator")))
            .expect_err("the route's own login is protected");
        assert!(
            error.0.contains("the login the registry reaches"),
            "{}",
            error.0
        );
    }

    #[test]
    fn another_account_on_that_host_is_deletable() {
        assert!(validate_deletable("scratch-lease-1", Some(&reached_as("operator"))).is_ok());
    }
}
