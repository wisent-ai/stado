//! The fixed remote programs of the scratch capability, and their answers.
//!
//! Each program is a compile-time constant fed to `/bin/sh -c` with quoted
//! arguments, the shape `host user create` already uses on this channel: no
//! registry value and no operator string is ever concatenated into shell, and
//! the privileged half is escalated exactly the way account creation escalates
//! it.
//!
//! Every program answers in the tab-delimited `STADO_*` marker protocol the
//! rest of the host channel speaks, so one parser family covers them all.

use serde_json::Value;

use super::lease::{record_dir, ScratchLease};
use crate::deploy::{shlex_quote, DeployError};

/// Marker prefix every scratch program prints.
pub const STATUS_PREFIX: &str = "STADO_SCRATCH";

/// Trust the new account with the keys that already reach this host.
///
/// The account's real home is read from the directory service rather than
/// assumed, because a mac's home is `/Users/<name>` only until someone creates
/// an account with `-home` pointing elsewhere, and a wrong guess here would
/// write an `authorized_keys` nobody reads — a lease that looks leased and
/// cannot be entered.
const TRUST_SCRIPT: &str = r#"set -eu

USERNAME=$1
SOURCE_KEYS=$2
OS_NAME=$(/usr/bin/uname -s)

if [ ! -s "$SOURCE_KEYS" ]; then
    echo "no authorized keys to copy from: $SOURCE_KEYS" >&2
    exit 65
fi

case "$OS_NAME" in
    Darwin)
        HOME_DIR=$(/usr/bin/dscl . -read "/Users/$USERNAME" NFSHomeDirectory | /usr/bin/awk '{print $2}')
        ;;
    Linux)
        HOME_DIR=$(/usr/bin/getent passwd "$USERNAME" | /usr/bin/cut -d: -f6)
        ;;
    *)
        echo "unsupported host OS: $OS_NAME" >&2
        exit 69
        ;;
esac

if [ -z "$HOME_DIR" ] || [ ! -d "$HOME_DIR" ]; then
    echo "account $USERNAME has no home directory to trust" >&2
    exit 66
fi

/usr/bin/install -d -m 700 -o "$USERNAME" "$HOME_DIR/.ssh"
/bin/cat "$SOURCE_KEYS" > "$HOME_DIR/.ssh/authorized_keys"
/usr/sbin/chown "$USERNAME" "$HOME_DIR/.ssh/authorized_keys"
/bin/chmod 600 "$HOME_DIR/.ssh/authorized_keys"

if [ ! -s "$HOME_DIR/.ssh/authorized_keys" ]; then
    echo "authorized keys for $USERNAME are empty after the copy" >&2
    exit 70
fi
printf 'STADO_SCRATCH\ttrusted\t%s\t%s\n' "$USERNAME" "$HOME_DIR""#;

/// Write one lease record in the login account's home. Unprivileged on
/// purpose: the record belongs to the account that took the lease.
const RECORD_SCRIPT: &str = r#"set -eu

RECORD_DIR=$1
RECORD_PATH=$2
RECORD_JSON=$3

/bin/mkdir -p "$RECORD_DIR"
printf '%s\n' "$RECORD_JSON" > "$RECORD_PATH"
printf 'STADO_SCRATCH\trecorded\t%s\n' "$RECORD_PATH""#;

/// Every lease this host holds, each with the two facts a record cannot state:
/// whether its account is still there, and where that account's home is. The
/// home comes from the directory service rather than a platform guess, because
/// a confirmation dialog that names the wrong directory is worse than one that
/// names none.
const LIST_SCRIPT: &str = r#"set -eu

RECORD_DIR=$1
OS_NAME=$(/usr/bin/uname -s)

[ -d "$RECORD_DIR" ] || exit 0
for record in "$RECORD_DIR"/*.json; do
    [ -f "$record" ] || continue
    name=$(/usr/bin/basename "$record" .json)
    home='-'
    if /usr/bin/id -u "$name" >/dev/null 2>&1; then
        account=present
        case "$OS_NAME" in
            Darwin)
                home=$(/usr/bin/dscl . -read "/Users/$name" NFSHomeDirectory 2>/dev/null | /usr/bin/awk '{print $2}')
                ;;
            *)
                home=$(/usr/bin/getent passwd "$name" | /usr/bin/cut -d: -f6)
                ;;
        esac
    else
        account=absent
    fi
    [ -n "$home" ] || home='-'
    body=$(/usr/bin/tr '\n\r\t' '   ' < "$record")
    printf 'STADO_SCRATCH\tlease\t%s\t%s\t%s\t%s\n' "$account" "$home" "$name" "$body"
done"#;

/// What the host holds for one name, asked after an operation claimed to
/// change it. Deliberately not the operation's own report: an account the
/// delete command says it removed and `id` still answers for is the exact
/// disagreement this probe exists to catch.
const STATE_SCRIPT: &str = r#"set -eu

NAME=$1
RECORD_PATH=$2

if /usr/bin/id -u "$NAME" >/dev/null 2>&1; then
    account=present
else
    account=absent
fi
home_path='-'
for candidate in "/Users/$NAME" "/home/$NAME"; do
    if [ -d "$candidate" ]; then
        home_path=$candidate
    fi
done
if [ "$home_path" = '-' ]; then
    home=absent
else
    home=present
fi
if [ -f "$RECORD_PATH" ]; then
    record=present
else
    record=absent
fi
printf 'STADO_SCRATCH\tstate\t%s\t%s\t%s\t%s\n' "$account" "$home" "$record" "$home_path""#;

/// Remove one lease record, and confirm it is gone.
const FORGET_SCRIPT: &str = r#"set -eu

RECORD_PATH=$1

/bin/rm -f "$RECORD_PATH"
if [ -e "$RECORD_PATH" ]; then
    echo "record survived removal: $RECORD_PATH" >&2
    exit 70
fi
printf 'STADO_SCRATCH\tforgotten\t%s\n' "$RECORD_PATH""#;

/// The privilege-escalating wrapper, identical in shape to account creation's.
fn privileged(script: &str, args: &[&str]) -> String {
    let invocation = invocation(script, args);
    format!(
        "if [ \"$(/usr/bin/id -u)\" -eq 0 ]; then exec {invocation}; else exec /usr/bin/sudo -n {invocation}; fi"
    )
}

fn invocation(script: &str, args: &[&str]) -> String {
    let quoted = args
        .iter()
        .map(|arg| shlex_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    format!("/bin/sh -c {} scratch {quoted}", shlex_quote(script))
}

/// Copy the login account's authorized keys into the leased account.
pub fn trust_command(username: &str, source_keys: &str) -> String {
    privileged(TRUST_SCRIPT, &[username, source_keys])
}

/// Write the lease record beside the others.
pub fn record_command(lease: &ScratchLease, home: &str) -> Result<String, DeployError> {
    let body = serde_json::to_string(lease)
        .map_err(|exc| DeployError(format!("lease record is not serializable: {exc}")))?;
    Ok(invocation(
        RECORD_SCRIPT,
        &[&record_dir(home), &lease.record_path(home), &body],
    ))
}

/// Read every lease this host holds.
pub fn list_command(home: &str) -> String {
    invocation(LIST_SCRIPT, &[&record_dir(home)])
}

/// Ask the host what it holds for one name.
pub fn state_command(name: &str, record_path: &str) -> String {
    invocation(STATE_SCRIPT, &[name, record_path])
}

/// Remove one record.
pub fn forget_command(record_path: &str) -> String {
    invocation(FORGET_SCRIPT, &[record_path])
}

/// One lease as the host reports it: the record, whether its account is still
/// there, and where that account's home is. A record that cannot be parsed is
/// kept as unreadable rather than dropped, because an unreadable record is a
/// leak the reaper must still be able to destroy.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLease {
    pub name: String,
    pub account_present: bool,
    pub home_path: Option<String>,
    pub lease: Option<ScratchLease>,
    pub unreadable: Option<String>,
}

/// The field value a program prints when it has nothing to report, kept out of
/// the parsed value so a caller never renders a dash as a path.
const NOTHING: &str = "-";

/// Parse the `lease` markers out of the list program's answer.
pub fn parse_leases(stdout: &str) -> Vec<HostLease> {
    let mut rows = Vec::new();
    for line in stdout.lines() {
        let fields = crate::deploy::host_channel::marker_fields(line);
        let [prefix, "lease", account, home, name, body @ ..] = fields.as_slice() else {
            continue;
        };
        if *prefix != STATUS_PREFIX {
            continue;
        }
        let text = body.join("\t");
        let (lease, unreadable) = match serde_json::from_str::<ScratchLease>(text.trim()) {
            Ok(parsed) if parsed.schema == super::lease::RECORD_SCHEMA => (Some(parsed), None),
            Ok(parsed) => (
                None,
                Some(format!("record declares schema '{}'", parsed.schema)),
            ),
            Err(exc) => (None, Some(exc.to_string())),
        };
        rows.push(HostLease {
            name: (*name).to_string(),
            account_present: *account == "present",
            home_path: reported(home),
            lease,
            unreadable,
        });
    }
    rows
}

/// A field the program filled in, or nothing.
fn reported(field: &str) -> Option<String> {
    if field.is_empty() || field == NOTHING {
        return None;
    }
    Some(field.to_string())
}

/// What the host said one name's account, home and record are.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostState {
    pub account: String,
    pub home: String,
    pub record: String,
    /// The home directory the probe actually found, when it found one.
    pub home_path: Option<String>,
}

impl HostState {
    /// Everything the operation promised to remove is gone.
    pub fn is_clear(&self) -> bool {
        self.account == "absent" && self.home == "absent" && self.record == "absent"
    }

    pub fn to_json(&self) -> Value {
        serde_json::json!({
            "account": self.account,
            "home": self.home,
            "record": self.record,
            "home_path": self.home_path,
        })
    }
}

/// Parse the `state` marker, refusing to invent a verdict when the probe
/// printed none: "the host says the account is gone" and "nobody asked the
/// host" are different facts.
pub fn parse_state(stdout: &str) -> Option<HostState> {
    stdout.lines().rev().find_map(|line| {
        let fields = crate::deploy::host_channel::marker_fields(line);
        let [prefix, "state", account, home, record, home_path] = fields.as_slice() else {
            return None;
        };
        if *prefix != STATUS_PREFIX {
            return None;
        }
        Some(HostState {
            account: (*account).to_string(),
            home: (*home).to_string(),
            record: (*record).to_string(),
            home_path: reported(home_path),
        })
    })
}

/// The value a single-marker program reported, by verb.
pub fn parse_marker(stdout: &str, verb: &str) -> Option<String> {
    stdout.lines().rev().find_map(|line| {
        let fields = crate::deploy::host_channel::marker_fields(line);
        let [prefix, printed, value @ ..] = fields.as_slice() else {
            return None;
        };
        if *prefix != STATUS_PREFIX || *printed != verb {
            return None;
        }
        Some(value.join("\t"))
    })
}
