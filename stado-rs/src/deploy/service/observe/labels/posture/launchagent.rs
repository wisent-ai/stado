use crate::deploy::service::*;

const USER_LAUNCHAGENT_SCRIPT: &str = r#"set -u
label=@LABEL@
action=@ACTION@
path="$HOME/Library/LaunchAgents/$label.plist"
uid=$(/usr/bin/id -u)
gui="gui/$uid"
user="user/$uid"
report() { printf 'STADO_USER_LAUNCHAGENT\t%s\t%s\n' "$1" "$2"; }
if [ "$(/usr/bin/uname -s)" != Darwin ]; then
  report refused "not a launchd host"
  exit 0
fi
if [ "$action" = inspect ] && [ ! -e "$path" ] && [ ! -L "$path" ]; then
  if /bin/launchctl print "$gui/$label" >/dev/null 2>&1 ||
     /bin/launchctl print "$user/$label" >/dev/null 2>&1; then
    report refused "$label is loaded without a restorable LaunchAgent plist"
  else
    report absent "$path"
  fi
  exit 0
fi
if [ "$action" != delete ]; then
  if [ ! -f "$path" ] || [ -L "$path" ]; then
    report refused "$path is not a regular non-symlink LaunchAgent plist"
    exit 0
  fi
  if ! /usr/bin/plutil -lint "$path" >/dev/null 2>&1; then
    report refused "$path is not a valid plist"
    exit 0
  fi
fi
case "$action" in
  check)
    report ready "$path"
    ;;
  inspect)
    report ready "$path"
    ;;
  restore)
    if /bin/launchctl print "$gui/$label" >/dev/null 2>&1 ||
       /bin/launchctl print "$user/$label" >/dev/null 2>&1; then
      report already_loaded "$label is already loaded"
      exit 0
    fi
    domain="$user"
    if /bin/launchctl print "$gui" >/dev/null 2>&1; then domain="$gui"; fi
    failure=$(/bin/launchctl bootstrap "$domain" "$path" 2>&1)
    code=$?
    if [ "$code" -eq 0 ] &&
       /bin/launchctl print "$domain/$label" >/dev/null 2>&1; then
      report restored "$domain/$label"
    else
      failure=$(printf '%s' "$failure" | /usr/bin/tr '\t\r\n' '   ')
      report failed "launchctl bootstrap $domain exited $code: $failure"
    fi
    ;;
  delete)
    if /bin/launchctl print "$gui/$label" >/dev/null 2>&1 ||
       /bin/launchctl print "$user/$label" >/dev/null 2>&1; then
      report refused "$label remains loaded; its plist was not removed"
      exit 0
    fi
    if [ ! -e "$path" ] && [ ! -L "$path" ]; then
      report absent "$path"
      exit 0
    fi
    if [ ! -f "$path" ] || [ -L "$path" ]; then
      report refused "$path is not a regular non-symlink LaunchAgent plist"
      exit 0
    fi
    if /bin/rm -f "$path" && [ ! -e "$path" ] && [ ! -L "$path" ]; then
      report removed "$path"
    else
      report failed "could not remove $path"
    fi
    ;;
  *)
    report refused "unsupported internal action"
    ;;
esac
"#;

async fn user_launchagent_action(
    target: &ComputeTarget,
    label: &str,
    action: &str,
    runner: &Runner,
) -> Result<(String, String), DeployError> {
    validate_unit_id(label)?;
    if !matches!(action, "check" | "inspect" | "restore" | "delete") {
        return Err(DeployError(format!(
            "unsupported internal LaunchAgent action {action:?}"
        )));
    }
    let script = USER_LAUNCHAGENT_SCRIPT
        .replace("@LABEL@", &format!("\"{}\"", quote_unit_path(label)?))
        .replace("@ACTION@", &format!("\"{}\"", action));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the LaunchAgent action did not complete",
        )));
    }
    output
        .stdout
        .lines()
        .find_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_USER_LAUNCHAGENT", state, detail] => {
                Some(((*state).trim().to_string(), (*detail).trim().to_string()))
            }
            _ => None,
        })
        .ok_or_else(|| {
            DeployError(format!(
                "{}: the LaunchAgent action reported nothing",
                target.name
            ))
        })
}

/// Prove that a legacy user LaunchAgent can be restored before taking it down.
pub async fn check_user_launchagent(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let (state, detail) = user_launchagent_action(target, label, "check", runner).await?;
    if state == "ready" {
        Ok(())
    } else {
        Err(DeployError(format!(
            "cannot supersede user LaunchAgent {label}: {detail}"
        )))
    }
}

/// Whether a same-named user LaunchAgent exists and can be restored on rollback.
pub async fn restorable_user_launchagent_exists(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<bool, DeployError> {
    let (state, detail) = user_launchagent_action(target, label, "inspect", runner).await?;
    match state.as_str() {
        "ready" => Ok(true),
        "absent" => Ok(false),
        _ => Err(DeployError(format!(
            "cannot supersede user LaunchAgent {label}: {detail}"
        ))),
    }
}

/// Restore a user LaunchAgent whose replacement failed readiness.
pub async fn restore_user_launchagent(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let (state, detail) = user_launchagent_action(target, label, "restore", runner).await?;
    if state == "restored" || state == "already_loaded" {
        Ok(())
    } else {
        Err(DeployError(format!(
            "could not restore user LaunchAgent {label}: {detail}"
        )))
    }
}

/// Delete an unloaded superseded user LaunchAgent's fixed plist path.
pub async fn delete_user_launchagent(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let (state, detail) = user_launchagent_action(target, label, "delete", runner).await?;
    if state == "removed" || state == "absent" {
        Ok(())
    } else {
        Err(DeployError(format!(
            "could not delete superseded user LaunchAgent {label}: {detail}"
        )))
    }
}
