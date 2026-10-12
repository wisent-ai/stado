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

/// The internal actions the LaunchAgent script carries out; the type is the
/// whole set, so no caller can name another. Serialized, it is the word the
/// script's `case` dispatches on.
#[derive(Clone, Copy, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum LaunchAgentAction {
    Check,
    Inspect,
    Restore,
    Delete,
}

/// What the script reports for an action, parsed once from its marker.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
enum LaunchAgentState {
    /// A regular, valid plist is in place.
    Ready,
    /// No plist and nothing loaded under the label.
    Absent,
    Restored,
    AlreadyLoaded,
    Removed,
    Refused,
    Failed,
}

async fn user_launchagent_action(
    target: &ComputeTarget,
    label: &str,
    action: LaunchAgentAction,
    runner: &Runner,
) -> Result<(LaunchAgentState, String), DeployError> {
    validate_unit_id(label)?;
    let action_word = serde_json::to_value(action)
        .map_err(|error| DeployError(format!("LaunchAgent action: {error}")))?;
    let script = USER_LAUNCHAGENT_SCRIPT
        .replace("@LABEL@", &format!("\"{}\"", quote_unit_path(label)?))
        .replace("@ACTION@", &action_word.to_string());
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError::unreachable(host_channel::last_error_line(
            &output,
            "the LaunchAgent action did not complete",
        )));
    }
    let (word, detail) = output
        .stdout
        .lines()
        .find_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_USER_LAUNCHAGENT", state, detail] => {
                Some(((*state).trim().to_string(), (*detail).trim().to_string()))
            }
            _ => None,
        })
        .ok_or_else(|| {
            DeployError::unreachable(format!(
                "{}: the LaunchAgent action reported nothing",
                target.name
            ))
        })?;
    let state =
        <LaunchAgentState as serde::Deserialize>::deserialize(serde_json::Value::String(word))
            .map_err(|error| {
                DeployError::unreachable(format!(
                    "{}: the LaunchAgent report state is malformed: {error}",
                    target.name
                ))
            })?;
    Ok((state, detail))
}

/// Prove that a legacy user LaunchAgent can be restored before taking it down.
pub async fn check_user_launchagent(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<(), DeployError> {
    let (state, detail) =
        user_launchagent_action(target, label, LaunchAgentAction::Check, runner).await?;
    if state == LaunchAgentState::Ready {
        Ok(())
    } else {
        Err(DeployError::unreachable(format!(
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
    let (state, detail) =
        user_launchagent_action(target, label, LaunchAgentAction::Inspect, runner).await?;
    match state {
        LaunchAgentState::Ready => Ok(true),
        LaunchAgentState::Absent => Ok(false),
        _ => Err(DeployError::unreachable(format!(
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
    let (state, detail) =
        user_launchagent_action(target, label, LaunchAgentAction::Restore, runner).await?;
    if matches!(
        state,
        LaunchAgentState::Restored | LaunchAgentState::AlreadyLoaded
    ) {
        Ok(())
    } else {
        Err(DeployError::unreachable(format!(
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
    let (state, detail) =
        user_launchagent_action(target, label, LaunchAgentAction::Delete, runner).await?;
    if matches!(state, LaunchAgentState::Removed | LaunchAgentState::Absent) {
        Ok(())
    } else {
        Err(DeployError::unreachable(format!(
            "could not delete superseded user LaunchAgent {label}: {detail}"
        )))
    }
}
