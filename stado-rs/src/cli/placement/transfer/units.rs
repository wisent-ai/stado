//! One concrete launchd or systemd unit, probed and acted on through a shared
//! shell preamble, so the Darwin domain and the systemd user session are
//! decided once rather than at every call site.

use std::num::NonZeroU32;

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
    let kind = STANDARD.encode(managed.kind.as_str().as_bytes());
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

/// A unit as its host's service manager reports it: whether its file is
/// installed, whether the manager holds it (launchd: bootstrapped; systemd:
/// active), and whether its process runs now (launchd: `state = running`;
/// systemd: active).
#[derive(Debug, Clone, Copy)]
pub(super) struct UnitStatus {
    pub(super) present: bool,
    pub(super) loaded: bool,
    pub(super) running: bool,
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
running=no
if [ -f "$unit_path" ]; then present=yes; fi
if [ "$os" = Darwin ]; then
  if listing=$(lc print "$domain/$unit" 2>/dev/null); then
    loaded=yes
    case "$listing" in *"state = running"*) running=yes ;; esac
  fi
else
  if systemctl_user is-active --quiet "$unit"; then loaded=yes; running=yes; fi
fi
printf 'STADO_PLACEMENT_UNIT\t%s\t%s\t%s\n' "$present" "$loaded" "$running"
"#
    );
    let output = run_host_script(target, &script, runner, "unit probe").await?;
    let line = marker_line(&output, "STADO_PLACEMENT_UNIT\t").ok_or_else(|| {
        CmdError::click(format!(
            "{}: unit probe returned no placement marker",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown)
    })?;
    let fields: Vec<&str> = line.split('\t').collect();
    let [_, present, loaded, running] = fields.as_slice() else {
        return Err(CmdError::click(format!(
            "{}: malformed unit probe marker: {line}",
            target.name
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    };
    Ok(UnitStatus {
        present: *present == "yes",
        loaded: *loaded == "yes",
        running: *running == "yes",
    })
}

/// The process a unit's service manager runs for it (launchd's `pid`,
/// systemd's `MainPID`) and the TCP ports that process listens on, as the
/// host's socket table states them (`lsof` on Darwin, `ss` on Linux). A
/// unit with no process has neither.
pub(super) struct Listening {
    pub(super) pid: Option<NonZeroU32>,
    pub(super) ports: Vec<u16>,
    /// Lines the socket table's reader wrote that are not sockets: its
    /// refusals, kept so they are shown rather than read as no listener.
    pub(super) unread: Vec<String>,
}

pub(super) async fn listening(
    target: &ComputeTarget,
    spec: &PlacementUnit,
    runner: &Runner,
) -> Result<Listening, CmdError> {
    let script = format!(
        "{}{}",
        unit_script_head(spec)?,
        r#"pid=
table=
if [ "$os" = Darwin ]; then
  if listing=$(lc print "$domain/$unit" 2>/dev/null); then
    pid=$(printf '%s\n' "$listing" | /usr/bin/sed -n 's/^[[:space:]]*pid = //p')
  fi
  if [ -n "$pid" ]; then table=$(/usr/sbin/lsof -nP -a -p "$pid" -iTCP -sTCP:LISTEN -Fn 2>&1 || :); fi
else
  pid=$(systemctl_user show --property MainPID --value "$unit")
  table=$(/usr/bin/ss -Hltnp 2>&1 || :)
fi
printf 'STADO_PLACEMENT_LISTEN\t%s\t%s\n' "$pid" "$(printf '%s' "$table" | /usr/bin/base64 | /usr/bin/tr -d '\n')"
"#
    );
    let output = run_host_script(target, &script, runner, "listener probe").await?;
    let refused = |detail: String| {
        CmdError::click(format!("{}: listener probe {detail}", target.name))
            .stating(crate::primitives::failure::FailureCode::InfraDown)
    };
    let line = marker_line(&output, "STADO_PLACEMENT_LISTEN\t")
        .ok_or_else(|| refused("returned no placement marker".to_string()))?;
    let fields: Vec<&str> = line.split('\t').collect();
    let [_, pid, table] = fields.as_slice() else {
        return Err(refused(format!("returned a malformed marker: {line}")));
    };
    let pid = match pid.is_empty() {
        true => None,
        false => NonZeroU32::new(
            pid.parse::<u32>()
                .map_err(|error| refused(format!("named process id {pid:?}: {error}")))?,
        ),
    };
    let table = STANDARD.decode(table).map_err(|error| {
        refused(format!(
            "returned a socket table that is not base64: {error}"
        ))
    })?;
    let mut found = Listening {
        pid,
        ports: Vec::new(),
        unread: Vec::new(),
    };
    let Some(pid) = pid else {
        return Ok(found);
    };
    let owner = format!("pid={pid},");
    for row in String::from_utf8_lossy(&table).lines() {
        // lsof -F writes one field per line, named by its first letter: p
        // the process, f the descriptor, n the address. ss writes a row per
        // socket whose first address is the local one and whose users field
        // names the owning process.
        let address = match row.chars().next() {
            Some('n') => Some(&row['n'.len_utf8()..]),
            Some('p') | Some('f') => None,
            _ => match row.strip_prefix("LISTEN") {
                Some(socket) => socket
                    .contains(&owner)
                    .then(|| socket.split_whitespace().find(|token| token.contains(':')))
                    .flatten(),
                None => {
                    found.unread.push(row.to_string());
                    None
                }
            },
        };
        let Some(address) = address else { continue };
        match address
            .rsplit_once(':')
            .map(|(_, port)| port.parse::<u16>())
        {
            Some(Ok(port)) => found.ports.push(port),
            Some(Err(_)) | None => found.unread.push(row.to_string()),
        }
    }
    Ok(found)
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
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }
    Ok(())
}
