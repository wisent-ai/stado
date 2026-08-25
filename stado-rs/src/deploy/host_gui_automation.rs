//! Report, grant, and revert the GUI-automation enablement of a
//! registry-managed macOS host.
//!
//! A headless mac needs a console session before any native UI test can run,
//! and getting one means autologin, an accessibility grant for the driver, and
//! remote-management access. Those were previously arranged by hand over ad-hoc
//! SSH, which left a host carrying an autologin password and a pre-seeded TCC
//! row with nothing in Stado that knew about it, could report it, or could take
//! it away. This module owns that state through the same approved channel
//! `host user create` uses.
//!
//! `status` reads, `grant-accessibility` performs the same per-user TCC write as
//! the System Settings switch, and `disable` reverts. Every mutation is
//! idempotent and reports the state read back from the host.

use std::time::Duration;

use crate::deploy::host_users::{ssh_argv, validate_ssh_target, SSH_TIMEOUT_SECONDS};
use crate::deploy::{shlex_quote, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Marker prefix of the remote script's report lines.
pub const STATUS_PREFIX: &str = "STADO_GUI\t";

/// Environment variable carrying the driver bundle id whose accessibility
/// grant should be revoked. Empty means "leave TCC alone" rather than guess.
pub const BUNDLE_ENV: &str = "STADO_GUI_BUNDLE";

/// Grant CuaDriver Accessibility to the console user, or to the remote-login
/// user when the host is sitting at the login window. The application must
/// already be installed and validly signed; the designated requirement stored
/// with the TCC row binds the grant to those signed bytes instead of trusting a
/// bundle identifier alone.
pub const REMOTE_GRANT_ACCESSIBILITY_SCRIPT: &str = r#"set -eu
app=/Applications/CuaDriver.app
login_user="${STADO_GUI_LOGIN_USER:-}"
console_user=$(/usr/bin/stat -f %Su /dev/console 2>/dev/null || true)
case "$console_user" in
  ""|root|loginwindow|_mbsetupuser) user="$login_user" ;;
  *) user="$console_user" ;;
esac
case "$user" in
  ""|root|loginwindow|_mbsetupuser)
    printf 'STADO_GUI\taccessibility\t%s\n' no-user
    exit 1
    ;;
esac
case "$user" in
  *[!A-Za-z0-9._-]*)
    printf 'STADO_GUI\taccessibility\t%s\n' invalid-user
    exit 1
    ;;
esac
if [ ! -d "$app" ]; then
  printf 'STADO_GUI\taccessibility\t%s\n' app-missing
  exit 1
fi
/usr/bin/codesign --verify --deep --strict "$app"
bundle=$(/usr/bin/defaults read "$app/Contents/Info" CFBundleIdentifier)
case "$bundle" in
  ""|*[!A-Za-z0-9._-]*)
    printf 'STADO_GUI\taccessibility\t%s\n' invalid-bundle
    exit 1
    ;;
esac
requirement=$(/usr/bin/codesign -dr - "$app" 2>&1)
requirement=${requirement#* => }
csreq=$(/usr/bin/csreq -r "$requirement" -b /dev/stdout | /usr/bin/xxd -p | /usr/bin/tr -d '\n')
if [ -z "$csreq" ]; then
  printf 'STADO_GUI\taccessibility\t%s\n' missing-code-requirement
  exit 1
fi
home="/Users/$user"
db="$home/Library/Application Support/com.apple.TCC/TCC.db"
if [ ! -f "$db" ]; then
  printf 'STADO_GUI\taccessibility\t%s\n' tcc-database-missing
  exit 1
fi
columns=$(/usr/bin/sqlite3 "$db" "SELECT group_concat(name, ',') FROM pragma_table_info('access');")
for required in service client client_type auth_value auth_reason auth_version csreq indirect_object_identifier_type indirect_object_identifier flags last_modified; do
  case ",$columns," in
    *",$required,"*) ;;
    *)
      printf 'STADO_GUI\taccessibility\tunsupported-tcc-schema:%s\n' "$required"
      exit 1
      ;;
  esac
done
backup_dir="$home/.stado/backups"
backup="$backup_dir/TCC.db.before-stado-accessibility"
/bin/mkdir -p "$backup_dir"
if [ ! -f "$backup" ]; then
  /usr/bin/sqlite3 "$db" ".backup '$backup'"
  /usr/sbin/chown "$user":staff "$backup"
  /bin/chmod 600 "$backup"
fi
/usr/bin/sqlite3 "$db" "BEGIN IMMEDIATE;
DELETE FROM access
 WHERE service = 'kTCCServiceAccessibility'
   AND client = '$bundle'
   AND client_type = 0;
INSERT INTO access (
  service, client, client_type, auth_value, auth_reason, auth_version,
  csreq, policy_id, indirect_object_identifier_type,
  indirect_object_identifier, indirect_object_code_identity, flags,
  last_modified
) VALUES (
  'kTCCServiceAccessibility', '$bundle', 0, 2, 3, 1,
  X'$csreq', NULL, 0, 'UNUSED', NULL, 0, strftime('%s','now')
);
COMMIT;"
granted=$(/usr/bin/sqlite3 "$db" "SELECT auth_value FROM access WHERE service = 'kTCCServiceAccessibility' AND client = '$bundle' AND client_type = 0 ORDER BY last_modified DESC LIMIT 1;")
if [ "$granted" != 2 ]; then
  printf 'STADO_GUI\taccessibility\t%s\n' write-not-observed
  exit 1
fi
printf 'STADO_GUI\taccessibility\t%s\n' granted
printf 'STADO_GUI\taccessibility-user\t%s\n' "$user"
printf 'STADO_GUI\taccessibility-client\t%s\n' "$bundle"
printf 'STADO_GUI\taccessibility-backup\t%s\n' "$backup"
"#;

/// Enumerate the enablement state. Every line is `STADO_GUI\t<item>\t<state>`.
pub const REMOTE_STATUS_SCRIPT: &str = r#"set -eu
autologin=$(/usr/bin/defaults read /Library/Preferences/com.apple.loginwindow autoLoginUser 2>/dev/null || true)
printf 'STADO_GUI\tautologin\t%s\n' "${autologin:-none}"

if [ -f /etc/kcpassword ]; then
  printf 'STADO_GUI\tkcpassword\t%s\n' present
else
  printf 'STADO_GUI\tkcpassword\t%s\n' absent
fi

ard=$(/usr/bin/defaults read /Library/Preferences/com.apple.RemoteManagement ARD_AllLocalUsers 2>/dev/null || true)
printf 'STADO_GUI\tremote-management-all-users\t%s\n' "${ard:-unset}"

vnc=$(/usr/bin/defaults read /Library/Preferences/com.apple.RemoteManagement VNCLegacyConnectionsEnabled 2>/dev/null || true)
printf 'STADO_GUI\tvnc-legacy\t%s\n' "${vnc:-unset}"

for path in /Applications/CuaDriver.app /Users/Shared/bin /Users/Shared/venv-stado; do
  if [ -e "$path" ]; then
    printf 'STADO_GUI\tartifact\t%s\n' "$path"
  fi
done

console=$(/usr/bin/stat -f %Su /dev/console 2>/dev/null || echo unknown)
printf 'STADO_GUI\tconsole\t%s\n' "$console"
case "$console" in
  ""|root|loginwindow|_mbsetupuser) user="${STADO_GUI_LOGIN_USER:-}" ;;
  *) user="$console" ;;
esac
if [ -d /Applications/CuaDriver.app ]; then
  bundle=$(/usr/bin/defaults read /Applications/CuaDriver.app/Contents/Info CFBundleIdentifier 2>/dev/null || true)
  db="/Users/$user/Library/Application Support/com.apple.TCC/TCC.db"
  if [ -n "$bundle" ] && [ -f "$db" ]; then
    value=$(/usr/bin/sqlite3 "$db" "SELECT auth_value FROM access WHERE service = 'kTCCServiceAccessibility' AND client = '$bundle' AND client_type = 0 ORDER BY last_modified DESC LIMIT 1;" 2>/dev/null || true)
    case "$value" in
      2) state=granted ;;
      "") state=not-set ;;
      *) state="refused:$value" ;;
    esac
    printf 'STADO_GUI\taccessibility\t%s\n' "$state"
    printf 'STADO_GUI\taccessibility-user\t%s\n' "$user"
    printf 'STADO_GUI\taccessibility-client\t%s\n' "$bundle"
  else
    printf 'STADO_GUI\taccessibility\t%s\n' unavailable
  fi
else
  printf 'STADO_GUI\taccessibility\t%s\n' app-missing
fi
"#;

/// Revert every item `status` reports. The TCC grant is revoked only for the
/// bundle id in `STADO_GUI_BUNDLE`; an empty value skips that step.
pub const REMOTE_DISABLE_SCRIPT: &str = r#"set -eu
bundle="${STADO_GUI_BUNDLE:-}"
kickstart=/System/Library/CoreServices/RemoteManagement/ARDAgent.app/Contents/Resources/kickstart
prefs=/Library/Preferences/com.apple.RemoteManagement

if /usr/bin/defaults read /Library/Preferences/com.apple.loginwindow autoLoginUser >/dev/null 2>&1; then
  /usr/bin/defaults delete /Library/Preferences/com.apple.loginwindow autoLoginUser || true
  printf 'STADO_GUI\tautologin\t%s\n' removed
else
  printf 'STADO_GUI\tautologin\t%s\n' absent
fi

if [ -f /etc/kcpassword ]; then
  /bin/rm -f /etc/kcpassword
  printf 'STADO_GUI\tkcpassword\t%s\n' removed
else
  printf 'STADO_GUI\tkcpassword\t%s\n' absent
fi

"$kickstart" -deactivate -configure -access -off >/dev/null 2>&1 || true
# kickstart deactivates the service but leaves behind the preferences it wrote,
# so a later status still reports the host as open to every local user and
# reachable over legacy VNC. Clear the keys as well as the service.
"$kickstart" -configure -clientopts -setvnclegacy -vnclegacy no >/dev/null 2>&1 || true
for key in ARD_AllLocalUsers ARD_AllLocalUsersPrivs VNCLegacyConnectionsEnabled; do
  /usr/bin/defaults delete "$prefs" "$key" >/dev/null 2>&1 || true
done
printf 'STADO_GUI\tremote-management\t%s\n' deactivated
printf 'STADO_GUI\tremote-management-prefs\t%s\n' cleared

if [ -n "$bundle" ]; then
  for home in /Users/*; do
    db="$home/Library/Application Support/com.apple.TCC/TCC.db"
    if [ -f "$db" ]; then
      /usr/bin/sqlite3 "$db" "DELETE FROM access WHERE client = '$bundle';" >/dev/null 2>&1 || true
      printf 'STADO_GUI\ttcc-revoked\t%s\n' "$db"
    fi
  done
fi

for path in /Applications/CuaDriver.app /Users/Shared/bin /Users/Shared/venv-stado; do
  if [ -e "$path" ]; then
    /bin/rm -rf "$path"
    printf 'STADO_GUI\tartifact-removed\t%s\n' "$path"
  fi
done
"#;

/// One host's report: the `(item, state)` pairs its script emitted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GuiAutomationReport {
    pub target: String,
    pub ssh_target: String,
    pub items: Vec<(String, String)>,
    pub error: Option<String>,
}

/// Wrap a script in the privilege escalation `host user create` uses: run it
/// directly when already root, otherwise through non-interactive sudo. The
/// original login user is captured before sudo so a host at the login window
/// still has an unambiguous per-user TCC database.
pub fn remote_command(script: &str, bundle: &str) -> String {
    let assignment = format!("{BUNDLE_ENV}={}", shlex_quote(bundle));
    let invocation = format!(
        "/usr/bin/env {assignment} STADO_GUI_LOGIN_USER=\"$login_user\" /bin/sh -c {}",
        shlex_quote(script)
    );
    format!(
        "login_user=$(/usr/bin/id -un); if [ \"$(/usr/bin/id -u)\" -eq 0 ]; then exec {invocation}; else exec /usr/bin/sudo -n {invocation}; fi"
    )
}

/// Parse the marker lines, preserving the order the host emitted them.
pub fn parse_report(stdout: &str) -> Vec<(String, String)> {
    let mut items = Vec::new();
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix(STATUS_PREFIX) else {
            continue;
        };
        let mut fields = rest.split('\t');
        let Some(item) = fields.next().filter(|item| !item.is_empty()) else {
            continue;
        };
        let state = fields.next().unwrap_or_default();
        items.push((item.to_string(), state.to_string()));
    }
    items
}

/// The SSH destination of a registry target, validated the same way account
/// provisioning validates it.
fn ssh_destination(target: &ComputeTarget) -> Result<String, DeployError> {
    let destination = target.ssh.as_deref().unwrap_or("");
    if destination.is_empty() {
        return Err(DeployError(format!(
            "target {} has no ssh destination in the registry",
            target.name
        )));
    }
    validate_ssh_target(destination)?;
    Ok(destination.to_string())
}

/// Run one script on one host and collect its report.
pub async fn run_on_host(
    target: &ComputeTarget,
    script: &str,
    bundle: &str,
    runner: &Runner,
) -> GuiAutomationReport {
    let ssh_target = match ssh_destination(target) {
        Ok(destination) => destination,
        Err(error) => {
            return GuiAutomationReport {
                target: target.name.clone(),
                ssh_target: String::new(),
                items: Vec::new(),
                error: Some(error.0),
            }
        }
    };
    let argv = ssh_argv(&ssh_target, &remote_command(script, bundle));
    let mut spec = CommandSpec::new(argv);
    spec.timeout = Some(Duration::from_secs(SSH_TIMEOUT_SECONDS));
    match runner(spec).await {
        Ok(output) if output.ok() => GuiAutomationReport {
            target: target.name.clone(),
            ssh_target,
            items: parse_report(&output.stdout),
            error: None,
        },
        Ok(output) => GuiAutomationReport {
            target: target.name.clone(),
            ssh_target,
            items: parse_report(&output.stdout),
            error: Some(output.detail().trim().to_string()),
        },
        Err(error) => GuiAutomationReport {
            target: target.name.clone(),
            ssh_target,
            items: Vec::new(),
            error: Some(error),
        },
    }
}
