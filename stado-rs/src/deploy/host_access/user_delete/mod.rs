//! Remove a local account from a registry-managed host.
//!
//! `host user create` had no counterpart, so an account provisioned for a
//! one-off task could only be removed by hand over ad-hoc SSH — which is how
//! a test account outlived its purpose on a managed mac. Deletion runs through
//! the same approved channel, validates the name with the same rule, and
//! refuses the accounts a host cannot survive losing.
//!
//! Which accounts those are is the host's own fact, read on the host: on Linux
//! an account whose uid is below the `UID_MIN` the host declares in
//! `/etc/login.defs`, on macOS an account whose home is not under `/Users`
//! (root, daemon, nobody and every `_` service account live elsewhere). A list
//! of three account names decided this before and protected none of the
//! dozens of service accounts a host carries. The login a host is reached by
//! is read from that host's own registry route.
//!
//! The home directory goes with the account unless `keep_home` is set; the
//! remote script reports which of the two it did.

use crate::deploy::host_users::validate_username;
use crate::deploy::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Marker prefix of the remote script's report line.
pub const STATUS_PREFIX: &str = "STADO_USER_DELETE\t";

/// The status the remote script reports for an account the host marks as a
/// system account.
const PROTECTED_STATUS: &str = "protected";

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
  # A login account's home is under /Users; system and service accounts
  # live in /var or nowhere.
  home_dir=$(/usr/bin/dscl . -read "/Users/$username" NFSHomeDirectory 2>/dev/null | /usr/bin/sed 's/^NFSHomeDirectory: *//')
  case "$home_dir" in
    /Users/*) ;;
    *)
      printf 'STADO_USER_DELETE\tprotected\t%s\t%s\n' "$os_name" "$username"
      exit
      ;;
  esac
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
  # The first uid of a login account is the host's own declaration; an
  # account below it, or a host that declares none, is not deleted.
  uid_min=$(/usr/bin/awk '$1 == "UID_MIN" { print $2 }' /etc/login.defs 2>/dev/null || true)
  uid=$(/usr/bin/id -u "$username")
  if [ -z "$uid_min" ] || [ "$uid" -lt "$uid_min" ]; then
    printf 'STADO_USER_DELETE\tprotected\t%s\t%s\n' "$os_name" "$username"
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

/// Reject the accounts that must not be removable through this command
/// before anything reaches the host: the login this host is reached by. The
/// host itself refuses its system accounts ([`PROTECTED_STATUS`]).
pub fn validate_deletable(
    username: &str,
    target: Option<&ComputeTarget>,
) -> Result<(), DeployError> {
    validate_username(username)?;
    if let Some(target) = target {
        if route_logins(target).iter().any(|login| login == username) {
            return Err(DeployError(format!(
                "refusing to delete {username}: it is the login the registry reaches {} with",
                target.name
            ))
            .stating(crate::primitives::failure::FailureCode::Refused));
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
        let [status, os_name, reported, ..] = fields.as_slice() else {
            continue;
        };
        if *reported == username {
            return Ok((status.to_string(), os_name.to_string()));
        }
    }
    Err(DeployError::unreachable(
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
        result.error = Some(error.message);
        return result;
    }
    if !target.has_ssh_connection() {
        result.error = Some(format!(
            "target {} has no SSH connection path in the registry",
            target.name
        ));
        return result;
    }

    let execution = host_channel::run_script_with_connection(
        target,
        &remote_command(username, keep_home),
        runner,
    )
    .await;
    let output = match execution {
        Ok((output, host_channel::UsedConnection::Ssh(connection))) => {
            result.ssh_target = connection.destination.to_string();
            Ok(output)
        }
        Ok((output, host_channel::UsedConnection::Local)) => Ok(output),
        Err(error) => Err(error.message),
    };
    match output {
        Ok(output) if output.ok() => match parse_status(&output.stdout, username) {
            Ok((status, os_name)) if status == PROTECTED_STATUS => {
                result.os_name = os_name;
                result.error = Some(format!(
                    "refusing to delete {username}: {} reports it as a system account (Linux: uid below the UID_MIN /etc/login.defs declares, or none declared; macOS: home not under /Users)",
                    target.name
                ));
            }
            Ok((status, os_name)) => {
                result.status = status;
                result.os_name = os_name;
            }
            Err(error) => result.error = Some(error.message),
        },
        Ok(output) => result.error = Some(output.detail().trim().to_string()),
        Err(error) => result.error = Some(error),
    }
    result
}
