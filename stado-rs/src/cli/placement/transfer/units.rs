//! One concrete launchd or systemd unit, probed and acted on through a shared
//! shell preamble, so the Darwin domain and the systemd user session are
//! decided once rather than at every call site.

use base64::{engine::general_purpose::STANDARD, Engine as _};

use super::{marker_line, run_host_script};
use crate::cli::placement::candidates::managed_unit;
use crate::cli::CmdError;
use crate::deploy::Runner;
use crate::placement::PlacementUnit;
use crate::targets::ComputeTarget;

fn unit_script_head(spec: &PlacementUnit) -> Result<String, CmdError> {
    let managed = managed_unit(spec)?;
    let unit = STANDARD.encode(managed.unit.as_bytes());
    let path = STANDARD.encode(managed.path.as_bytes());
    let kind = STANDARD.encode(managed.kind.as_bytes());
    Ok(format!(
        r#"set -eu
case "$(/usr/bin/uname -s)" in Darwin) decode=-D ;; *) decode=--decode ;; esac
unit=$(printf '%s' '{unit}' | /usr/bin/base64 "$decode")
unit_path=$(printf '%s' '{path}' | /usr/bin/base64 "$decode")
expected_kind=$(printf '%s' '{kind}' | /usr/bin/base64 "$decode")
os=$(/usr/bin/uname -s)
uid=$(/usr/bin/id -u)
if [ "$os" = Darwin ]; then
  [ "$expected_kind" = launchd ] || {{ printf 'expected systemd, found Darwin\n' >&2; exit 65; }}
  case "$unit_path" in
    /Library/LaunchDaemons/*) domain=system ;;
    *)
      if /bin/launchctl print "gui/$uid" >/dev/null 2>&1; then domain="gui/$uid"; else domain="user/$uid"; fi
      ;;
  esac
  lc() {{
    if [ "$domain" = system ] && [ "$uid" -ne 0 ]; then /usr/bin/sudo -n /bin/launchctl "$@"; else /bin/launchctl "$@"; fi
  }}
elif [ "$os" = Linux ]; then
  [ "$expected_kind" = systemd ] || {{ printf 'expected launchd, found Linux\n' >&2; exit 65; }}
  systemctl_user() {{ /usr/bin/systemctl --user "$@"; }}
else
  printf 'unsupported OS: %s\n' "$os" >&2
  exit 65
fi
"#
    ))
}

#[derive(Debug, Clone, Copy)]
pub(super) struct UnitStatus {
    pub(super) present: bool,
    pub(super) loaded: bool,
}

pub(super) async fn probe_unit(
    target: &ComputeTarget,
    spec: &PlacementUnit,
    runner: &Runner,
) -> Result<UnitStatus, CmdError> {
    let script = format!(
        "{}{}",
        unit_script_head(spec)?,
        r#"present=no
loaded=no
if [ -f "$unit_path" ]; then present=yes; fi
if [ "$os" = Darwin ]; then
  if lc print "$domain/$unit" >/dev/null 2>&1; then loaded=yes; fi
else
  if systemctl_user is-active --quiet "$unit"; then loaded=yes; fi
fi
printf 'STADO_PLACEMENT_UNIT\t%s\t%s\n' "$present" "$loaded"
"#
    );
    let output = run_host_script(target, &script, runner, "unit probe").await?;
    let line = marker_line(&output, "STADO_PLACEMENT_UNIT\t").ok_or_else(|| {
        CmdError::click(format!(
            "{}: unit probe returned no placement marker",
            target.name
        ))
    })?;
    let fields: Vec<&str> = line.split('\t').collect();
    if fields.len() != 3 {
        return Err(CmdError::click(format!(
            "{}: malformed unit probe marker",
            target.name
        )));
    }
    Ok(UnitStatus {
        present: fields[1] == "yes",
        loaded: fields[2] == "yes",
    })
}

#[derive(Debug, Clone, Copy)]
pub(super) enum UnitAction {
    Stop,
    Start,
    Retire,
}

impl UnitAction {
    fn name(self) -> &'static str {
        match self {
            Self::Stop => "stop",
            Self::Start => "start",
            Self::Retire => "retire",
        }
    }
}

pub(super) async fn act_on_unit(
    target: &ComputeTarget,
    spec: &PlacementUnit,
    action: UnitAction,
    runner: &Runner,
) -> Result<(), CmdError> {
    let managed = managed_unit(spec)?;
    let body = match action {
        UnitAction::Stop => {
            r#"if [ "$os" = Darwin ]; then
  lc bootout "$domain/$unit" >/dev/null 2>&1 || true
else
  systemctl_user stop "$unit"
fi
"#
        }
        UnitAction::Start => {
            r#"if [ "$os" = Darwin ]; then
  lc bootout "$domain/$unit" >/dev/null 2>&1 || true
  lc enable "$domain/$unit" >/dev/null
  lc bootstrap "$domain" "$unit_path"
  lc print "$domain/$unit" >/dev/null
else
  systemctl_user daemon-reload
  systemctl_user enable "$unit" >/dev/null
  systemctl_user restart "$unit"
  systemctl_user is-active --quiet "$unit"
fi
"#
        }
        UnitAction::Retire => {
            r#"if [ "$os" = Darwin ]; then
  lc bootout "$domain/$unit" >/dev/null 2>&1 || true
  lc disable "$domain/$unit" >/dev/null
else
  systemctl_user disable --now "$unit"
fi
"#
        }
    };
    let script = format!(
        "{}{}printf 'STADO_PLACEMENT_ACTION\\t{}\\tok\\n'\n",
        unit_script_head(spec)?,
        body,
        action.name()
    );
    let output = run_host_script(
        target,
        &script,
        runner,
        &format!("{} {}", action.name(), managed.unit),
    )
    .await?;
    if marker_line(
        &output,
        &format!("STADO_PLACEMENT_ACTION\t{}\tok", action.name()),
    )
    .is_none()
    {
        return Err(CmdError::click(format!(
            "{}: {} {} returned no success marker",
            target.name,
            action.name(),
            managed.unit
        )));
    }
    Ok(())
}
