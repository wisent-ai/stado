use crate::deploy::service::*;

const AUTOSTART_SCRIPT: &str = r#"set -u
label=@LABEL@
action=@ACTION@
requested_scope=@REQUESTED_SCOPE@
report() { printf 'STADO_AUTOSTART\t%s\t%s\n' "$1" "$2"; }
os=$(/usr/bin/uname -s)
if [ "$os" = Darwin ]; then
  uid=$(/usr/bin/id -u)
  launch=/bin/launchctl
  disabled_state() {
    state_scope=$1
    if [ "$state_scope" = system ]; then
      raw=$(/usr/bin/sudo -n "$launch" print-disabled system 2>/dev/null) || return 1
    else
      raw=$("$launch" print-disabled "$state_scope" 2>/dev/null) || return 1
    fi
    printf '%s\n' "$raw" | /usr/bin/awk -v wanted="\"$label\"" '
      $1 == wanted && $2 == "=>" {
        value=$3; gsub(/[;,]/, "", value); print value; found=1; exit
      }
      END { if (!found) print "false" }'
  }
  present_scope() {
    candidate=$1
    case "$candidate" in
      system)
        [ -e "/Library/LaunchDaemons/$label.plist" ] ||
          /usr/bin/sudo -n "$launch" print "system/$label" >/dev/null 2>&1
        ;;
      gui/*|user/*)
        [ -e "$HOME/Library/LaunchAgents/$label.plist" ] ||
          [ -e "/Library/LaunchAgents/$label.plist" ] ||
          "$launch" print "$candidate/$label" >/dev/null 2>&1
        ;;
      *) return 1 ;;
    esac
  }
  if [ "$requested_scope" = any ]; then
    scopes="system gui/$uid user/$uid"
  else
    scopes=$requested_scope
  fi
  found=no
  for candidate in $scopes; do
    present_scope "$candidate" || continue
    found=yes
    if [ "$action" != inspect ]; then
      if [ "$candidate" = system ]; then
        detail=$(/usr/bin/sudo -n "$launch" "$action" "$candidate/$label" 2>&1)
      else
        detail=$("$launch" "$action" "$candidate/$label" 2>&1)
      fi
      rc=$?
      if [ "$rc" -ne 0 ]; then
        report refused "$candidate status=$rc $detail"
        exit 0
      fi
    fi
    disabled=$(disabled_state "$candidate") || {
      report refused "$candidate print-disabled failed"
      exit 0
    }
    case "$disabled" in
      true|disabled) state=disabled ;;
      false|enabled) state=enabled ;;
      *) report refused "$candidate returned invalid disabled state $disabled"; exit 0 ;;
    esac
    if { [ "$action" = enable ] && [ "$state" != enabled ]; } ||
       { [ "$action" = disable ] && [ "$state" != disabled ]; }; then
      report refused "$candidate did not reach requested $action state"
      exit 0
    fi
    report "$candidate" "$state"
  done
  if [ "$found" = no ]; then report absent "$requested_scope"; fi
  exit 0
fi
if [ "$os" != Linux ]; then
  report refused "unsupported service manager on $os"
  exit 0
fi
uid=$(/usr/bin/id -u)
systemdctl() {
  manager_scope=$1
  shift
  if [ "$manager_scope" = system ]; then
    if [ "$uid" = 0 ]; then /usr/bin/systemctl "$@"; else /usr/bin/sudo -n /usr/bin/systemctl "$@"; fi
  else
    runtime="/run/user/$uid"
    /usr/bin/env XDG_RUNTIME_DIR="$runtime" \
      DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
      /usr/bin/systemctl --user "$@"
  fi
}
if [ "$requested_scope" = any ]; then scopes="system user"; else scopes=$requested_scope; fi
found=no
for candidate in $scopes; do
  load=$(systemdctl "$candidate" show --property=LoadState --value -- "$label" 2>/dev/null) || continue
  [ "$load" != not-found ] || continue
  found=yes
  if [ "$action" != inspect ]; then
    detail=$(systemdctl "$candidate" "$action" -- "$label" 2>&1)
    rc=$?
    if [ "$rc" -ne 0 ]; then report refused "$candidate status=$rc $detail"; exit 0; fi
  fi
  enabled=$(systemdctl "$candidate" is-enabled -- "$label" 2>/dev/null || true)
  case "$enabled" in
    enabled|enabled-runtime|linked|linked-runtime|alias) state=enabled ;;
    disabled|static|indirect|generated|transient|masked|masked-runtime) state=disabled ;;
    *) report refused "$candidate returned invalid enabled state ${enabled:-empty}"; exit 0 ;;
  esac
  if { [ "$action" = enable ] && [ "$state" != enabled ]; } ||
     { [ "$action" = disable ] && [ "$state" != disabled ]; }; then
    report refused "$candidate did not reach requested $action state"
    exit 0
  fi
  report "$candidate" "$state"
done
if [ "$found" = no ]; then report absent "$requested_scope"; fi
"#;

fn autostart_script(label: &str, action: &str, scope: &str) -> Result<String, DeployError> {
    validate_unit_id(label)?;
    if !matches!(action, "inspect" | "enable" | "disable") {
        return Err(DeployError(format!("invalid autostart action {action:?}")));
    }
    let valid_scope = matches!(scope, "any" | "system" | "user")
        || scope
            .strip_prefix("gui/")
            .or_else(|| scope.strip_prefix("user/"))
            .is_some_and(|uid| !uid.is_empty() && uid.bytes().all(|byte| byte.is_ascii_digit()));
    if !valid_scope {
        return Err(DeployError(format!("invalid autostart scope {scope:?}")));
    }
    Ok(AUTOSTART_SCRIPT
        .replace("@LABEL@", &format!("\"{}\"", quote_unit_path(label)?))
        .replace("@ACTION@", action)
        .replace("@REQUESTED_SCOPE@", scope))
}

/// Read every installed/loaded init-system scope's persistent boot state for
/// one exact unit. `true` means enabled after reboot.
pub async fn label_autostart(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<BTreeMap<String, bool>, DeployError> {
    let output =
        host_channel::run_script(target, &autostart_script(label, "inspect", "any")?, runner)
            .await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "autostart inspection failed",
        )));
    }
    let mut states = BTreeMap::new();
    for line in output.stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_AUTOSTART", "refused", detail] => {
                return Err(DeployError((*detail).to_string()));
            }
            ["STADO_AUTOSTART", scope, "enabled"] => {
                states.insert((*scope).to_string(), true);
            }
            ["STADO_AUTOSTART", scope, "disabled"] => {
                states.insert((*scope).to_string(), false);
            }
            _ => {}
        }
    }
    Ok(states)
}

/// Persist one exact init-system scope's boot state and verify the manager
/// reports the requested state.
pub async fn set_label_autostart(
    target: &ComputeTarget,
    label: &str,
    scope: &str,
    enabled: bool,
    runner: &Runner,
) -> Result<(), DeployError> {
    let action = if enabled { "enable" } else { "disable" };
    let output =
        host_channel::run_script(target, &autostart_script(label, action, scope)?, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "autostart mutation failed",
        )));
    }
    let expected = if enabled { "enabled" } else { "disabled" };
    let found = output.stdout.lines().any(|line| {
        matches!(
            host_channel::marker_fields(line).as_slice(),
            ["STADO_AUTOSTART", found_scope, found_state]
                if *found_scope == scope && *found_state == expected
        )
    });
    if !found {
        return Err(DeployError(format!(
            "{label} {scope} did not verify as {expected}"
        )));
    }
    Ok(())
}
