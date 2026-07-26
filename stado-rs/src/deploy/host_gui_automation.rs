//! Report and revert the GUI-automation enablement of a registry-managed
//! macOS host.
//!
//! A headless mac needs a console session before any native UI test can run,
//! and getting one means autologin, an accessibility grant for the driver, and
//! remote-management access. Those were previously arranged by hand over ad-hoc
//! SSH, which left a host carrying an autologin password and a pre-seeded TCC
//! row with nothing in stado that knew about it, could report it, or could take
//! it away. This module is that missing half: the state is enumerated and
//! reverted through the same approved channel `host user create` uses.
//!
//! `status` reads; `disable` reverts. Both are idempotent, and both name every
//! item they touched so the report is the evidence. The remote scripts report
//! raw values and never compare them, so interpretation stays on this side.

use std::time::Duration;

use crate::deploy::host_users::{ssh_argv, validate_ssh_target, SSH_TIMEOUT_SECONDS};
use crate::deploy::{shlex_quote, CommandSpec, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Marker prefix of the remote script's report lines.
pub const STATUS_PREFIX: &str = "STADO_GUI\t";

/// Environment variable carrying the driver bundle id whose accessibility
/// grant should be revoked. Empty means "leave TCC alone" rather than guess.
pub const BUNDLE_ENV: &str = "STADO_GUI_BUNDLE";

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
/// bundle id travels as an environment assignment, never in the script text.
pub fn remote_command(script: &str, bundle: &str) -> String {
    let assignment = format!("{BUNDLE_ENV}={}", shlex_quote(bundle));
    let invocation = format!(
        "/usr/bin/env {assignment} /bin/sh -c {}",
        shlex_quote(script)
    );
    format!(
        "if [ \"$(/usr/bin/id -u)\" -eq 0 ]; then exec {invocation}; else exec /usr/bin/sudo -n {invocation}; fi"
    )
}

/// Parse the marker lines, preserving the order the host emitted them.
pub fn parse_report(stdout: &str) -> Vec<(String, String)> {
    let mut items = Vec::new();
    for line in stdout.lines() {
        let Some(rest) = line.strip_prefix(STATUS_PREFIX) else { continue };
        let mut fields = rest.split('\t');
        let Some(item) = fields.next().filter(|item| !item.is_empty()) else { continue };
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
