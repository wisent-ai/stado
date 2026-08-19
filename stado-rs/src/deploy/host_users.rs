//! Create standard or administrator accounts on registry-managed hosts.
//!
//! Port of `stado/deploy/host_users.py`. Passwords travel only on SSH
//! stdin. They are never placed in the local SSH argv, the remote command
//! string, registry data, or command output.

use std::sync::LazyLock;
use std::time::Duration;

use super::{shlex_quote, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

static USERNAME_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[a-z][a-z0-9_-]{0,30}$").expect("static regex compiles"));
static SSH_TARGET_RE: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"^[A-Za-z0-9_.:@\[\]-]+$").expect("static regex compiles"));

/// Python `_STATUS_PREFIX`.
pub const STATUS_PREFIX: &str = "STADO_USER\t";

/// Python `runner(..., timeout=120)` for the per-host ssh call.
pub const SSH_TIMEOUT_SECONDS: u64 = 120;

/// Python `_REMOTE_CREATE_SCRIPT` (byte-exact; verified against the
/// checked-in golden rendered by the Python module).
pub const REMOTE_CREATE_SCRIPT: &str = r#"set -eu

USERNAME=$1
FULL_NAME=$2
REQUESTED_SHELL=$3
MAKE_ADMIN=$4
REQUIRE_PASSWORD_CHANGE=$5
OS_NAME=$(/usr/bin/uname -s)

if /usr/bin/id "$USERNAME" >/dev/null 2>&1; then
    printf 'STADO_USER\texists\t%s\t%s\n' "$OS_NAME" "$USERNAME"
    exit 0
fi

IFS= read -r PASSWORD
if [ -z "$PASSWORD" ]; then
    echo "initial password is empty" >&2
    exit 65
fi

case "$OS_NAME" in
    Darwin)
        SHELL_PATH=${REQUESTED_SHELL:-/bin/zsh}
        if [ ! -x "$SHELL_PATH" ]; then
            echo "requested shell is not executable: $SHELL_PATH" >&2
            exit 66
        fi
        if [ "$MAKE_ADMIN" = 1 ]; then
            /usr/sbin/sysadminctl -addUser "$USERNAME" \
                -fullName "$FULL_NAME" -home "/Users/$USERNAME" \
                -shell "$SHELL_PATH" -password "$PASSWORD" -admin >/dev/null
        else
            /usr/sbin/sysadminctl -addUser "$USERNAME" \
                -fullName "$FULL_NAME" -home "/Users/$USERNAME" \
                -shell "$SHELL_PATH" -password "$PASSWORD" >/dev/null
        fi
        /usr/sbin/createhomedir -c -u "$USERNAME" >/dev/null
        if [ "$REQUIRE_PASSWORD_CHANGE" = 1 ]; then
            /usr/bin/pwpolicy -u "$USERNAME" -setpolicy "newPasswordRequired=1" >/dev/null
        fi
        ;;
    Linux)
        SHELL_PATH=${REQUESTED_SHELL:-/bin/bash}
        if [ ! -x "$SHELL_PATH" ]; then
            echo "requested shell is not executable: $SHELL_PATH" >&2
            exit 66
        fi
        /usr/sbin/useradd --create-home --comment "$FULL_NAME" \
            --shell "$SHELL_PATH" "$USERNAME"
        printf '%s:%s\n' "$USERNAME" "$PASSWORD" | /usr/sbin/chpasswd
        if [ "$MAKE_ADMIN" = 1 ]; then
            if /usr/bin/getent group sudo >/dev/null 2>&1; then
                /usr/sbin/usermod --append --groups sudo "$USERNAME"
            elif /usr/bin/getent group wheel >/dev/null 2>&1; then
                /usr/sbin/usermod --append --groups wheel "$USERNAME"
            else
                echo "neither sudo nor wheel administrator group exists" >&2
                exit 67
            fi
        fi
        if [ "$REQUIRE_PASSWORD_CHANGE" = 1 ]; then
            /usr/bin/chage --lastday 0 "$USERNAME"
        fi
        ;;
    *)
        echo "unsupported host OS: $OS_NAME" >&2
        exit 69
        ;;
esac

if ! /usr/bin/id "$USERNAME" >/dev/null 2>&1; then
    echo "account creation command returned without creating $USERNAME" >&2
    exit 70
fi
printf 'STADO_USER\tcreated\t%s\t%s\n' "$OS_NAME" "$USERNAME""#;

/// Python `HostUserResult`: one host's outcome.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostUserResult {
    pub target: String,
    pub ssh: String,
    pub status: String,
    pub os_name: String,
    pub detail: String,
}

impl HostUserResult {
    /// Python `ok` property: created / exists / planned all count.
    pub fn ok(&self) -> bool {
        matches!(self.status.as_str(), "created" | "exists" | "planned")
    }
}

/// Validate a portable, non-system macOS/Linux account name
/// (Python `validate_username`).
pub fn validate_username(username: &str) -> Result<(), DeployError> {
    if !USERNAME_RE.is_match(username) {
        return Err(DeployError(
            "username must start with a lowercase letter, contain only \
             lowercase letters, digits, '_' or '-', and be at most 31 characters"
                .to_string(),
        ));
    }
    Ok(())
}

/// Python `_validate_text`.
fn validate_text(value: &str, label: &str, max_length: usize) -> Result<(), DeployError> {
    if value.is_empty()
        || value.chars().count() > max_length
        || value.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n'))
    {
        return Err(DeployError(format!(
            "{label} must be 1-{max_length} characters without control newlines"
        )));
    }
    Ok(())
}

/// Python `_validate_shell`: empty is allowed (host OS default).
pub fn validate_shell(shell: &str) -> Result<(), DeployError> {
    if shell.is_empty() {
        return Ok(());
    }
    validate_text(shell, "shell", 255)?;
    if !shell.starts_with('/') {
        return Err(DeployError("shell must be an absolute path".to_string()));
    }
    Ok(())
}

/// Python `_validate_password`.
pub fn validate_password(password: &str) -> Result<(), DeployError> {
    let length = password.chars().count();
    if !(8..=1024).contains(&length) {
        return Err(DeployError(
            "initial password must be between 8 and 1024 characters".to_string(),
        ));
    }
    if password.chars().any(|ch| matches!(ch, '\0' | '\r' | '\n')) {
        return Err(DeployError(
            "initial password must not contain NUL or newlines".to_string(),
        ));
    }
    Ok(())
}

/// Python `_validate_ssh_target`.
pub fn validate_ssh_target(ssh_target: &str) -> Result<(), DeployError> {
    if ssh_target.is_empty() || ssh_target.starts_with('-') || !SSH_TARGET_RE.is_match(ssh_target) {
        return Err(DeployError(format!(
            "unsafe SSH destination in registry: {}",
            super::py_str_repr(ssh_target)
        )));
    }
    Ok(())
}

/// Python `_select_targets`: exactly one of --target / --all; every
/// selected target must be kind=local with a safe SSH destination.
pub fn select_targets<'a>(
    targets: &[&'a ComputeTarget],
    names: &[String],
    all_targets: bool,
) -> Result<Vec<&'a ComputeTarget>, DeployError> {
    // Python: `if bool(names) == all_targets`.
    if names.is_empty() != all_targets {
        return Err(DeployError(
            "provide one or more --target values, or --all, but not both".to_string(),
        ));
    }

    let selected: Vec<&ComputeTarget> = if !names.is_empty() {
        let mut selected = Vec::new();
        let mut seen: Vec<&str> = Vec::new();
        let mut missing: Vec<&str> = Vec::new();
        for name in names {
            match targets.iter().find(|t| t.name == *name) {
                Some(target) => {
                    if !seen.contains(&name.as_str()) {
                        seen.push(name);
                        selected.push(*target);
                    }
                }
                None => missing.push(name),
            }
        }
        if !missing.is_empty() {
            return Err(DeployError(format!(
                "registry target not found: {}",
                missing.join(", ")
            )));
        }
        selected
    } else {
        let selected: Vec<&ComputeTarget> = targets
            .iter()
            .filter(|target| {
                target.is_provider(crate::capabilities::ProviderId::Local)
                    && target.ssh.as_deref().is_some_and(|ssh| !ssh.is_empty())
            })
            .copied()
            .collect();
        if selected.is_empty() {
            return Err(DeployError(
                "registry contains no SSH-managed local targets".to_string(),
            ));
        }
        selected
    };

    for target in &selected {
        if !target.is_provider(crate::capabilities::ProviderId::Local) {
            return Err(DeployError(format!(
                "target {} is not kind=local",
                super::py_str_repr(&target.name)
            )));
        }
        if target.ssh.as_deref().unwrap_or("").is_empty() {
            return Err(DeployError(format!(
                "target {} has no SSH destination",
                super::py_str_repr(&target.name)
            )));
        }
        validate_ssh_target(target.ssh.as_deref().unwrap_or(""))?;
    }
    Ok(selected)
}

/// Python `_remote_command`: the privilege-escalating wrapper around the
/// quoted create script.
pub fn remote_command(
    username: &str,
    full_name: &str,
    shell: &str,
    admin: bool,
    require_password_change: bool,
) -> String {
    let args = [
        "stado-create-user",
        username,
        full_name,
        shell,
        if admin { "1" } else { "0" },
        if require_password_change { "1" } else { "0" },
    ];
    let invocation = format!(
        "/bin/sh -c {} {}",
        shlex_quote(REMOTE_CREATE_SCRIPT),
        args.iter()
            .map(|arg| shlex_quote(arg))
            .collect::<Vec<_>>()
            .join(" ")
    );
    format!(
        "if [ \"$(/usr/bin/id -u)\" -eq 0 ]; then exec {invocation}; else exec /usr/bin/sudo -n {invocation}; fi"
    )
}

/// Python's ssh argv in `provision_users` (note the -o order: BatchMode,
/// StrictHostKeyChecking, ConnectTimeout — different from host_recovery).
pub fn ssh_argv(ssh_target: &str, command: &str) -> Vec<String> {
    vec![
        "ssh".to_string(),
        "-o".to_string(),
        "BatchMode=yes".to_string(),
        "-o".to_string(),
        "StrictHostKeyChecking=accept-new".to_string(),
        "-o".to_string(),
        "ConnectTimeout=15".to_string(),
        ssh_target.to_string(),
        command.to_string(),
    ]
}

/// Python `_parse_status`: the LAST valid marker line wins
/// (`reversed(stdout.splitlines())`).
pub fn parse_status(stdout: &str, username: &str) -> Result<(String, String), DeployError> {
    for line in stdout.lines().rev() {
        if !line.starts_with(STATUS_PREFIX) {
            continue;
        }
        let fields: Vec<&str> = line.split('\t').collect();
        if fields.len() == 4 && matches!(fields[1], "created" | "exists") && fields[3] == username {
            return Ok((fields[1].to_string(), fields[2].to_string()));
        }
    }
    Err(DeployError(
        "remote host did not return a valid account status marker".to_string(),
    ))
}

/// Keyword options of Python `provision_users`.
#[derive(Debug, Default, Clone)]
pub struct ProvisionOptions<'a> {
    pub username: &'a str,
    pub password: Option<&'a str>,
    pub target_names: &'a [String],
    pub all_targets: bool,
    pub full_name: Option<&'a str>,
    pub shell: &'a str,
    pub admin: bool,
    pub require_password_change: bool,
    pub dry_run: bool,
}

/// Python `provision_users`: create one account on each selected registry
/// host and return every outcome.
pub async fn provision_users(
    options: &ProvisionOptions<'_>,
    targets: &[&ComputeTarget],
    runner: &Runner,
) -> Result<Vec<HostUserResult>, DeployError> {
    validate_username(options.username)?;
    let full_name = options.full_name.unwrap_or(options.username);
    validate_text(full_name, "full name", 255)?;
    validate_shell(options.shell)?;
    if !options.dry_run {
        let Some(password) = options.password else {
            return Err(DeployError("initial password is required".to_string()));
        };
        validate_password(password)?;
    }

    let selected = select_targets(targets, options.target_names, options.all_targets)?;

    let mut results: Vec<HostUserResult> = Vec::new();
    for target in selected {
        let name = target.name.clone();
        let ssh_target = target.ssh.clone().unwrap_or_default();
        validate_ssh_target(&ssh_target)?;
        if options.dry_run {
            results.push(HostUserResult {
                target: name,
                ssh: ssh_target,
                status: "planned".to_string(),
                os_name: String::new(),
                detail: String::new(),
            });
            continue;
        }

        let command = remote_command(
            options.username,
            full_name,
            options.shell,
            options.admin,
            options.require_password_change,
        );
        let spec = CommandSpec {
            argv: ssh_argv(&ssh_target, &command),
            stdin: Some(format!("{}\n", options.password.unwrap_or(""))),
            timeout: Some(Duration::from_secs(SSH_TIMEOUT_SECONDS)),
        };
        let completed = match runner(spec).await {
            Ok(completed) => completed,
            Err(exc) => {
                results.push(HostUserResult {
                    target: name,
                    ssh: ssh_target,
                    status: "failed".to_string(),
                    os_name: String::new(),
                    detail: exc,
                });
                continue;
            }
        };

        if !completed.ok() {
            let detail_text = if !completed.stderr.is_empty() {
                completed.stderr.as_str()
            } else if !completed.stdout.is_empty() {
                completed.stdout.as_str()
            } else {
                ""
            };
            let fallback;
            let detail_text = if detail_text.is_empty() {
                fallback = format!("ssh exit {}", completed.code);
                fallback.as_str()
            } else {
                detail_text
            };
            let trimmed = detail_text.trim();
            let detail: String = trimmed
                .chars()
                .skip(trimmed.chars().count().saturating_sub(2000))
                .collect();
            results.push(HostUserResult {
                target: name,
                ssh: ssh_target,
                status: "failed".to_string(),
                os_name: String::new(),
                detail,
            });
            continue;
        }
        match parse_status(&completed.stdout, options.username) {
            Ok((status, os_name)) => results.push(HostUserResult {
                target: name,
                ssh: ssh_target,
                status,
                os_name,
                detail: String::new(),
            }),
            Err(exc) => results.push(HostUserResult {
                target: name,
                ssh: ssh_target,
                status: "failed".to_string(),
                os_name: String::new(),
                detail: exc.0,
            }),
        }
    }
    Ok(results)
}
