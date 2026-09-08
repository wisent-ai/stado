use crate::deploy::service::*;

/// Boot one exact init-system unit out of the system or calling-account scope.
///
/// The last gap in the world-to-declaration direction. `service list
/// --undeclared` can name a unit the registry never declared, and
/// `space file remove` can delete its unit file — but `service stop` refuses a
/// unit with no declaration to resolve. A loaded unit whose file is already
/// gone otherwise has no owner left that can stop it.
///
/// On launchd, system jobs use the same `sudo -n` grant `ENSURE_BODY` uses.
/// User jobs are removed from both explicit `gui/<uid>` and `user/<uid>`
/// domains, and each domain is proven absent before success. On systemd,
/// system jobs use that same non-interactive privilege path and user jobs use
/// the calling account's explicit runtime bus. The exact requested unit is
/// disabled with `--now`; its identity, disabled state, and inactive state are
/// then read back before success. Neither branch removes a unit file.
///
/// `scope` exists because the system scope is tried first and returns. The same
/// name can identify distinct system and user jobs on either service manager,
/// so `User` is how an operator retires an undeclared duplicate without
/// touching its canonical system sibling. A unit the selected manager does not
/// hold is `absent`, not an error: repeated cleanup is safe.
const BOOTOUT_SCRIPT: &str = r#"set -u
label=@LABEL@
scope=@SCOPE@
report() { printf 'STADO_BOOTOUT\t%s\t%s\n' "$1" "$2"; }
os=$(/usr/bin/uname -s)
if [ "$os" = Darwin ]; then
  if [ "$scope" != user ] \
    && /usr/bin/sudo -n /bin/launchctl print "system/$label" >/dev/null 2>&1; then
    if ! /usr/bin/sudo -n /bin/launchctl bootout "system/$label" 2>/dev/null; then
      report refused "sudo -n launchctl bootout system/$label was refused"
      exit 0
    fi
    attempts=0
    while /usr/bin/sudo -n /bin/launchctl print "system/$label" >/dev/null 2>&1; do
      attempts=$((attempts + 1))
      if [ "$attempts" -ge 150 ]; then
        report refused "system/$label remained loaded after bootout"
        exit 0
      fi
      /bin/sleep 0.1
    done
    report booted_out "system/$label"
    exit 0
  fi
  uid=$(/usr/bin/id -u)
  removed=
  for domain in "gui/$uid" "user/$uid"; do
    if [ "$scope" = system ]; then continue; fi
    if /bin/launchctl print "$domain/$label" >/dev/null 2>&1; then
      if ! /bin/launchctl bootout "$domain/$label" 2>/dev/null; then
        report refused "launchctl bootout $domain/$label was refused"
        exit 0
      fi
      attempts=0
      while /bin/launchctl print "$domain/$label" >/dev/null 2>&1; do
        attempts=$((attempts + 1))
        if [ "$attempts" -ge 150 ]; then
          report refused "$domain/$label remained loaded after bootout"
          exit 0
        fi
        /bin/sleep 0.1
      done
      removed="$removed $domain/$label"
    fi
  done
  if [ -n "$removed" ]; then
    report booted_out "${removed# }"
  else
    report absent "launchd holds no $scope job for $label (system, gui/$uid and user/$uid all read empty)"
  fi
  exit 0
fi
if [ "$os" != Linux ]; then
  report refused "unsupported service manager on $os"
  exit 0
fi

uid=$(/usr/bin/id -u)
if [ -x /usr/bin/sudo ]; then sudo_bin=/usr/bin/sudo; else sudo_bin=/bin/sudo; fi
systemd_refuse() {
  refusal=$(printf '%s' "$2" | /usr/bin/tr '\t\r\n' ' ' | /usr/bin/cut -c1-300)
  if [ -n "$refusal" ]; then
    report refused "$1: $refusal"
  else
    report refused "$1"
  fi
  exit 0
}
systemdctl() {
  manager_scope=$1
  shift
  if [ "$manager_scope" = system ]; then
    if [ "$uid" = 0 ]; then
      /usr/bin/systemctl "$@"
    else
      "$sudo_bin" -n /usr/bin/systemctl "$@"
    fi
    return
  fi
  runtime="/run/user/$uid"
  /usr/bin/env \
    XDG_RUNTIME_DIR="$runtime" \
    DBUS_SESSION_BUS_ADDRESS="unix:path=$runtime/bus" \
    /usr/bin/systemctl --user "$@"
}
systemd_probe() {
  probe_scope=$1
  probe_output=$(systemdctl "$probe_scope" show --property=Id --property=LoadState -- "$label" 2>&1)
  probe_rc=$?
  if [ "$probe_rc" -ne 0 ]; then
    systemd_refuse "cannot inspect systemd $probe_scope/$label (status $probe_rc)" "$probe_output"
  fi
  probe_load=$(printf '%s\n' "$probe_output" | /usr/bin/awk -F= '$1 == "LoadState" { print $2; exit }')
  probe_id=$(printf '%s\n' "$probe_output" | /usr/bin/awk -F= '$1 == "Id" { sub(/^[^=]*=/, ""); print; exit }')
  case "$probe_load" in
    not-found) return 1 ;;
    loaded|masked) ;;
    error|bad-setting)
      systemd_refuse "systemd $probe_scope/$label has unusable load state $probe_load" "$probe_output"
      ;;
    *)
      systemd_refuse "systemd $probe_scope/$label returned unexpected load state ${probe_load:-empty}" "$probe_output"
      ;;
  esac
  if [ "$probe_id" != "$label" ]; then
    systemd_refuse "systemd $probe_scope name $label resolves to ${probe_id:-no unit id}; refusing a non-exact unit" ""
  fi
  return 0
}

selected=
if [ "$scope" != user ] && systemd_probe system; then selected=system; fi
if [ -z "$selected" ] && [ "$scope" != system ] && systemd_probe user; then selected=user; fi
if [ -z "$selected" ]; then
  case "$scope" in
    system|user) report absent "systemd $scope manager holds no exact unit named $label" ;;
    *) report absent "systemd system and user managers hold no exact unit named $label" ;;
  esac
  exit 0
fi

disable_output=$(systemdctl "$selected" disable --now -- "$label" 2>&1)
disable_rc=$?
if [ "$disable_rc" -ne 0 ]; then
  systemd_refuse "systemctl $selected disable --now $label failed (status $disable_rc)" "$disable_output"
fi

attempts=0
while :; do
  state_output=$(systemdctl "$selected" show --property=Id --property=LoadState --property=ActiveState -- "$label" 2>&1)
  state_rc=$?
  if [ "$state_rc" -ne 0 ]; then
    systemd_refuse "cannot verify systemd $selected/$label after disable (status $state_rc)" "$state_output"
  fi
  load_state=$(printf '%s\n' "$state_output" | /usr/bin/awk -F= '$1 == "LoadState" { print $2; exit }')
  active_state=$(printf '%s\n' "$state_output" | /usr/bin/awk -F= '$1 == "ActiveState" { print $2; exit }')
  state_id=$(printf '%s\n' "$state_output" | /usr/bin/awk -F= '$1 == "Id" { sub(/^[^=]*=/, ""); print; exit }')
  if [ "$load_state" = not-found ]; then
    active_state=not-found
    break
  fi
  if [ "$state_id" != "$label" ]; then
    systemd_refuse "systemd $selected/$label changed identity to ${state_id:-no unit id} during verification" "$state_output"
  fi
  if [ "$active_state" = inactive ]; then break; fi
  attempts=$((attempts + 1))
  if [ "$attempts" -ge 150 ]; then
    systemd_refuse "systemd $selected/$label remained ${active_state:-unknown} after disable --now" "$state_output"
  fi
  /bin/sleep 0.1
done

enabled_output=$(systemdctl "$selected" is-enabled -- "$label" 2>&1)
enabled_rc=$?
enabled_state=$(printf '%s\n' "$enabled_output" | /usr/bin/awk 'NF { state=$0 } END { print state }')
case "$enabled_state" in
  disabled|static|indirect|generated|transient|masked|masked-runtime|not-found) ;;
  enabled|enabled-runtime|linked|linked-runtime|alias)
    systemd_refuse "systemd $selected/$label remained enabled after disable --now" "$enabled_output"
    ;;
  *)
    systemd_refuse "cannot verify that systemd $selected/$label is disabled (status $enabled_rc)" "$enabled_output"
    ;;
esac
report booted_out "systemd $selected/$label is inactive and not enabled ($enabled_state)"
"#;

/// Which init-system scope a bootout may act in.
///
/// `Any` preserves the historical behaviour on both supported service
/// managers: system first, and the calling account only when the system scope
/// holds no exact unit by that name. The explicit variants distinguish names
/// that exist in both scopes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootoutScope {
    Any,
    System,
    User,
}

impl BootoutScope {
    /// The word the remote program compares against.
    pub fn word(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::System => "system",
            Self::User => "user",
        }
    }

    /// Parse an operator's `--domain`. `None` is [`Self::Any`].
    pub fn parse(value: Option<&str>) -> Result<Self, DeployError> {
        match value.map(str::trim) {
            None | Some("") | Some("any") => Ok(Self::Any),
            Some("system") => Ok(Self::System),
            Some("user") => Ok(Self::User),
            Some(other) => Err(DeployError(format!(
                "{other:?} is not an init-system scope: system, user, or any"
            ))),
        }
    }
}

/// Run [`BOOTOUT_SCRIPT`] for one exact unit name. Returns `(state, detail)`.
pub async fn bootout_label(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
) -> Result<(String, String), DeployError> {
    validate_unit_id(label)?;
    if label.contains('/') {
        return Err(DeployError(format!(
            "unit {} is not one exact launchd label or systemd unit name",
            py_str_repr(label)
        )));
    }
    let script = BOOTOUT_SCRIPT
        .replace("@LABEL@", &format!("\"{}\"", quote_unit_path(label)?))
        .replace("@SCOPE@", scope.word());
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the bootout did not complete",
        )));
    }
    output
        .stdout
        .lines()
        .find_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_BOOTOUT", state, detail] => {
                Some(((*state).trim().to_string(), (*detail).trim().to_string()))
            }
            _ => None,
        })
        .ok_or_else(|| DeployError(format!("{}: the bootout reported nothing", target.name)))
}
