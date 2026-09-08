//! The remote side: the byte-exact create script, the status marker it
//! prints, the privilege-escalating wrapper that runs it, the ssh argv that
//! carries it, and the parser that reads the marker back.

use crate::deploy::{shlex_quote, DeployError};

/// Python `_STATUS_PREFIX`.
pub const STATUS_PREFIX: &str = "STADO_USER\t";

/// Python's `runner` deadline, in seconds, for the per-host ssh call.
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
