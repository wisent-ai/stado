//! Giving a renamed product its old units back when the new unit did not
//! start.
//!
//! `stado service ensure` retires a catalog product's `retired_units` before
//! it starts the product's one unit, because both bind the same listener.
//! Retiring boots the old unit out and withdraws its autostart, so when the
//! new unit then fails, the host is left running neither, and nothing starts
//! the old one again: a vault whose new unit never got a unit file answers
//! no read on that host until somebody notices. A failed start puts back
//! exactly what this run took away.

use crate::deploy::service::*;

const LOAD_SCRIPT: &str = r#"set -u
label=@LABEL@
report() { printf 'STADO_LOAD\t%s\t%s\n' "$1" "$2"; }
uid=$(/usr/bin/id -u)
case "$(/usr/bin/uname -s)" in
  Darwin)
    launch=/bin/launchctl
    if [ -f "/Library/LaunchDaemons/$label.plist" ]; then
      domain=system
      path="/Library/LaunchDaemons/$label.plist"
      run() { /usr/bin/sudo -n "$launch" "$@"; }
    elif [ -f "$HOME/Library/LaunchAgents/$label.plist" ]; then
      domain="user/$uid"
      if "$launch" print "gui/$uid" >/dev/null 2>&1; then domain="gui/$uid"; fi
      path="$HOME/Library/LaunchAgents/$label.plist"
      run() { "$launch" "$@"; }
    else
      report absent "no unit file for $label in /Library/LaunchDaemons or $HOME/Library/LaunchAgents"
      exit 0
    fi
    if run print "$domain/$label" >/dev/null 2>&1; then
      report loaded "$domain/$label was already loaded"
      exit 0
    fi
    failure=$(run bootstrap "$domain" "$path" 2>&1)
    code=$?
    if [ "$code" -eq 0 ] && run print "$domain/$label" >/dev/null 2>&1; then
      report loaded "$domain/$label from $path"
    else
      failure=$(printf '%s' "$failure" | /usr/bin/tr '\t\r\n' '   ')
      report failed "launchctl bootstrap $domain $path exited $code: $failure"
    fi
    ;;
  Linux)
    for scope in system user; do
      if [ "$scope" = system ]; then
        if [ "$uid" = 0 ]; then ctl() { /usr/bin/systemctl "$@"; }; else ctl() { /usr/bin/sudo -n /usr/bin/systemctl "$@"; }; fi
      else
        ctl() { /usr/bin/env XDG_RUNTIME_DIR="/run/user/$uid" DBUS_SESSION_BUS_ADDRESS="unix:path=/run/user/$uid/bus" /usr/bin/systemctl --user "$@"; }
      fi
      load=$(ctl show --property=LoadState --value -- "$label" 2>/dev/null) || continue
      [ "$load" != not-found ] || continue
      failure=$(ctl start -- "$label" 2>&1)
      code=$?
      if [ "$code" -eq 0 ] && ctl is-active --quiet -- "$label"; then
        report loaded "$scope $label"
      else
        failure=$(printf '%s' "$failure" | /usr/bin/tr '\t\r\n' '   ')
        report failed "systemctl start $label in the $scope manager exited $code: $failure"
      fi
      exit 0
    done
    report absent "neither systemd manager knows $label"
    ;;
  *)
    report failed "unsupported service manager on $(/usr/bin/uname -s)"
    ;;
esac
"#;

/// One unit [`retire_units_reversibly`] took down, and the scopes whose
/// autostart it withdrew, which are the scopes it gives back.
#[derive(Debug, Clone)]
pub struct Reversible {
    pub unit: String,
    pub scopes: Vec<String>,
}

/// Retire every unit `replacement` lists in `retired_units`, as
/// [`retire_units`] does, remembering for each unit it touched the autostart
/// scopes it is about to withdraw. A unit whose autostart cannot be read is
/// not touched at all and reported `failed`: without its scopes it could not
/// be given back. A unit that failed half way is still reversible, because
/// it may already be booted out.
pub async fn retire_units_reversibly(
    target: &ComputeTarget,
    replacement: &crate::deploy::service_catalog::CatalogService,
    runner: &Runner,
) -> (Vec<PredecessorRetirement>, Vec<Reversible>) {
    let mut retirements = Vec::with_capacity(replacement.retired_units.len());
    let mut reversible = Vec::with_capacity(replacement.retired_units.len());
    for unit in &replacement.retired_units {
        let scopes = match label_autostart(target, unit, runner).await {
            Ok(states) => states
                .into_iter()
                .filter_map(|(scope, enabled)| enabled.then_some(scope))
                .collect(),
            Err(error) => {
                retirements.push(PredecessorRetirement {
                    unit: unit.clone(),
                    state: "failed".to_string(),
                    detail: format!(
                        "its autostart could not be read, so it was left running: {error}"
                    ),
                });
                continue;
            }
        };
        let (state, detail) = match retire_label(target, unit, runner).await {
            Ok(outcome) => outcome,
            Err(error) => ("failed".to_string(), error.to_string()),
        };
        if state != "absent" {
            reversible.push(Reversible {
                unit: unit.clone(),
                scopes,
            });
        }
        retirements.push(PredecessorRetirement {
            unit: unit.clone(),
            state,
            detail,
        });
    }
    (retirements, reversible)
}

/// Undo [`retire_units_reversibly`]. `replacement_label` names the new unit
/// when it was started and failed: it is booted out first, so a
/// crash-looping new unit cannot hold the listener. `None` when the
/// replacement was never started. Each unit then gets its autostart back in
/// the scopes it was withdrawn from and is loaded again. One sentence per
/// step, each naming what it did or the exact failure; nothing here is
/// silent.
pub async fn reinstate_units(
    target: &ComputeTarget,
    replacement_label: Option<&str>,
    units: &[Reversible],
    runner: &Runner,
) -> Vec<String> {
    if units.is_empty() {
        return Vec::new();
    }
    let mut said = Vec::with_capacity(units.len() + 1);
    if let Some(replacement_label) = replacement_label {
        said.push(
            match bootout_label(target, replacement_label, BootoutScope::Any, runner).await {
                Ok((state, detail)) => format!("{replacement_label}: {state} ({detail})"),
                Err(error) => format!("{replacement_label} could not be booted out: {error}"),
            },
        );
    }
    for reversible in units {
        let unit = &reversible.unit;
        let mut enabled = Vec::with_capacity(reversible.scopes.len());
        let mut refused = Vec::new();
        for scope in &reversible.scopes {
            match set_label_autostart(target, unit, scope, true, runner).await {
                Ok(()) => enabled.push(scope.as_str()),
                Err(error) => refused.push(format!("{scope}: {error}")),
            }
        }
        let loaded = match load_label(target, unit, runner).await {
            Ok((state, detail)) => format!("{state} ({detail})"),
            Err(error) => format!("failed ({error})"),
        };
        let mut sentence = format!("{unit} given back: {loaded}");
        if !enabled.is_empty() {
            sentence.push_str(&format!(
                "; autostart enabled again in {}",
                enabled.join(", ")
            ));
        }
        if !refused.is_empty() {
            sentence.push_str(&format!("; autostart refused in {}", refused.join("; ")));
        }
        said.push(sentence);
    }
    said
}

/// Load one exact unit from its unit file, in the domain that file belongs
/// to. `(state, detail)` with `loaded`, `absent` or `failed`.
pub async fn load_label(
    target: &ComputeTarget,
    label: &str,
    runner: &Runner,
) -> Result<(String, String), DeployError> {
    validate_unit_id(label)?;
    let script = LOAD_SCRIPT.replace("@LABEL@", &format!("\"{}\"", quote_unit_path(label)?));
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the unit load did not complete",
        )));
    }
    output
        .stdout
        .lines()
        .find_map(|line| match host_channel::marker_fields(line).as_slice() {
            ["STADO_LOAD", state, detail] => {
                Some(((*state).trim().to_string(), (*detail).trim().to_string()))
            }
            _ => None,
        })
        .ok_or_else(|| DeployError(format!("{}: the unit load reported nothing", target.name)))
}
