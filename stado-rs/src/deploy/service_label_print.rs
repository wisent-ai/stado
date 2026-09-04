//! `stado service label-print` — ask the host init system what it holds under
//! one named unit, in one named domain, and print only fixed scalar facts.
//!
//! Enumeration cannot find a loaded unit whose file was deleted, and it cannot
//! explain a systemd service another unit keeps starting. This command asks for
//! the exact operator-supplied identity instead. On launchd it reports the
//! fixed `pid`, state, exit, path, program and argument fields. On systemd it
//! reports the matching fixed properties, including restart and trigger links.
//! Neither branch reads an environment property.
//!
//! It signals nothing, loads nothing and stops nothing. `service bootout` or
//! `service remove` are the commands that act; this one only answers.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::service::{quote_unit_path, validate_unit_id, BootoutScope};
use super::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Read-only init-system query. Only named scalar properties leave the host;
/// neither launchctl's environment block nor systemd's Environment property is
/// requested.
const LABEL_PRINT_SCRIPT: &str = "set -u
os=$(/usr/bin/uname -s)
label=@LABEL@
scope=@SCOPE@
uid=$(/usr/bin/id -u)
found=no
if [ \"$os\" = Darwin ]; then
  case \"$scope\" in
    system) domains='system' ;;
    user)   domains=\"user/$uid gui/$uid\" ;;
    *)      domains=\"system user/$uid gui/$uid\" ;;
  esac
  for domain in $domains; do
    block=$(/bin/launchctl print \"$domain/$label\" 2>/dev/null) || continue
    if [ -z \"$block\" ]; then continue; fi
    found=yes
    printf 'STADO_LABEL_DOMAIN\\t%s\\n' \"$domain\"
    printf '%s\\n' \"$block\" | /usr/bin/awk -F' = ' '
      { key=$1; sub(/^[ \\t]+/, \"\", key); sub(/[ \\t]+$/, \"\", key) }
      key == \"pid\" || key == \"state\" || key == \"last exit code\" || key == \"runs\" || key == \"path\" {
        value=$2
        sub(/^[ \\t]+/, \"\", value); sub(/[ \\t]+$/, \"\", value)
        printf \"STADO_LABEL_FIELD\\t%s\\t%s\\n\", key, value
      }'
    printf '%s\\n' \"$block\" | /usr/bin/awk '
      /^[ \\t]*program[ \\t]*=/ { line=$0; sub(/^[^=]*=[ \\t]*/, \"\", line); printf \"STADO_LABEL_FIELD\\tprogram\\t%s\\n\", line }
      /^[ \\t]*arguments[ \\t]*=[ \\t]*\\{/ { collecting=1; argv=\"\"; next }
      collecting && /^[ \\t]*\\}/ { collecting=0; sub(/^ /, \"\", argv); printf \"STADO_LABEL_FIELD\\targuments\\t%s\\n\", argv; next }
      collecting { line=$0; sub(/^[ \\t]+/, \"\", line); sub(/[ \\t]+$/, \"\", line); if (line != \"\") argv = argv \" \" line }'
    break
  done
elif [ \"$os\" = Linux ]; then
  case \"$scope\" in
    system) domains='system' ;;
    user)   domains='user' ;;
    *)      domains='user system' ;;
  esac
  for domain in $domains; do
    if [ \"$domain\" = system ]; then
      if [ \"$uid\" = 0 ]; then
        block=$(/usr/bin/systemctl show \"$label\" --property=LoadState,ActiveState,SubState,MainPID,UnitFileState,FragmentPath,ExecStart,Restart,Triggers,TriggeredBy,PartOf 2>/dev/null) || continue
      else
        block=$(/usr/bin/sudo -n /usr/bin/systemctl show \"$label\" --property=LoadState,ActiveState,SubState,MainPID,UnitFileState,FragmentPath,ExecStart,Restart,Triggers,TriggeredBy,PartOf 2>/dev/null) || continue
      fi
    else
      runtime=\"/run/user/$uid\"
      block=$(/usr/bin/env XDG_RUNTIME_DIR=\"$runtime\" DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" /usr/bin/systemctl --user show \"$label\" --property=LoadState,ActiveState,SubState,MainPID,UnitFileState,FragmentPath,ExecStart,Restart,Triggers,TriggeredBy,PartOf 2>/dev/null) || continue
    fi
    if printf '%s\\n' \"$block\" | /usr/bin/awk -F= '$1 == \"LoadState\" && $2 == \"not-found\" { found=1 } END { exit !found }'; then
      continue
    fi
    found=yes
    printf 'STADO_LABEL_DOMAIN\\t%s\\n' \"$domain\"
    printf '%s\\n' \"$block\" | /usr/bin/awk -F= '
      $1 == \"MainPID\" { printf \"STADO_LABEL_FIELD\\tpid\\t%s\\n\", $2 }
      $1 == \"SubState\" { printf \"STADO_LABEL_FIELD\\tstate\\t%s\\n\", $2 }
      $1 == \"UnitFileState\" { printf \"STADO_LABEL_FIELD\\tunit file state\\t%s\\n\", $2 }
      $1 == \"FragmentPath\" { printf \"STADO_LABEL_FIELD\\tpath\\t%s\\n\", $2 }
      $1 == \"ExecStart\" { line=$0; sub(/^[^=]*=/, \"\", line); printf \"STADO_LABEL_FIELD\\targuments\\t%s\\n\", line }
      $1 == \"Restart\" { printf \"STADO_LABEL_FIELD\\trestart\\t%s\\n\", $2 }
      $1 == \"Triggers\" { printf \"STADO_LABEL_FIELD\\ttriggers\\t%s\\n\", $2 }
      $1 == \"TriggeredBy\" { printf \"STADO_LABEL_FIELD\\ttriggered by\\t%s\\n\", $2 }
      $1 == \"PartOf\" { printf \"STADO_LABEL_FIELD\\tpart of\\t%s\\n\", $2 }'
    break
  done
else
  printf 'STADO_LABEL_UNSUPPORTED\\t%s\\n' \"$os\"
fi
printf 'STADO_LABEL_DONE\\t%s\\n' \"$found\"
";

/// What an init system holds under one exact service identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelState {
    pub host: String,
    pub label: String,
    /// The domain the label was found in, when it was found at all.
    pub domain: Option<String>,
    pub pid: Option<String>,
    pub state: Option<String>,
    pub last_exit_code: Option<String>,
    pub runs: Option<String>,
    /// The unit file the init system loaded the job from. A loaded job whose
    /// file has since been deleted still reports the last known path.
    pub path: Option<String>,
    pub program: Option<String>,
    pub arguments: Option<String>,
    pub unit_file_state: Option<String>,
    pub restart: Option<String>,
    pub triggers: Option<String>,
    pub triggered_by: Option<String>,
    pub part_of: Option<String>,
    /// Set when the host runs neither launchd nor systemd, naming its OS.
    pub unsupported: Option<String>,
}

impl LabelState {
    /// Did the host's supported init system answer for this identity?
    pub fn loaded(&self) -> bool {
        self.domain.is_some()
    }

    /// The program the job actually runs, preferring the full argv over the
    /// bare program path: every stado unit executes the same binary and the
    /// subcommand is the whole difference between an agent and a resolver.
    pub fn runs(&self) -> Option<&str> {
        self.arguments
            .as_deref()
            .filter(|argv| !argv.is_empty())
            .or(self.program.as_deref())
    }

    pub fn to_json(&self) -> Value {
        json!({
            "host": self.host,
            "label": self.label,
            "loaded": self.loaded(),
            "domain": self.domain,
            "pid": self.pid,
            "state": self.state,
            "last_exit_code": self.last_exit_code,
            "runs": self.runs_field(),
            "path": self.path,
            "program": self.program,
            "arguments": self.arguments,
            "unit_file_state": self.unit_file_state,
            "restart": self.restart,
            "triggers": self.triggers,
            "triggered_by": self.triggered_by,
            "part_of": self.part_of,
            "unsupported": self.unsupported,
        })
    }

    /// How many times launchd has started the job; absent on systemd.
    fn runs_field(&self) -> Option<&str> {
        self.runs.as_deref()
    }
}

/// Ask one host what it holds under one label.
///
/// Signals nothing. This reads only the host's init-system state.
pub async fn print_label(
    target: &ComputeTarget,
    label: &str,
    scope: BootoutScope,
    runner: &Runner,
) -> Result<LabelState, DeployError> {
    validate_unit_id(label)?;
    let script = LABEL_PRINT_SCRIPT
        .replace("@LABEL@", &format!("\"{}\"", quote_unit_path(label)?))
        .replace("@SCOPE@", scope.word());
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(host_channel::last_error_line(
            &output,
            "the label print did not complete",
        )));
    }
    Ok(parse_label_print(&target.name, label, &output.stdout))
}

/// Turn the marker stream into a state. Pure — covered by unit tests.
pub fn parse_label_print(host: &str, label: &str, stdout: &str) -> LabelState {
    let mut state = LabelState {
        host: host.to_string(),
        label: label.to_string(),
        ..LabelState::default()
    };
    for line in stdout.lines() {
        match host_channel::marker_fields(line).as_slice() {
            ["STADO_LABEL_UNSUPPORTED", system] => {
                state.unsupported = Some((*system).trim().to_string());
            }
            ["STADO_LABEL_DOMAIN", domain] => {
                state.domain = Some((*domain).trim().to_string());
            }
            ["STADO_LABEL_FIELD", key, value] => {
                let value = (*value).trim().to_string();
                if value.is_empty() {
                    continue;
                }
                match (*key).trim() {
                    "pid" => state.pid = Some(value),
                    "state" => state.state = Some(value),
                    "last exit code" => state.last_exit_code = Some(value),
                    "runs" => state.runs = Some(value),
                    "path" => state.path = Some(value),
                    "program" => state.program = Some(value),
                    "arguments" => state.arguments = Some(value),
                    "unit file state" => state.unit_file_state = Some(value),
                    "restart" => state.restart = Some(value),
                    "triggers" => state.triggers = Some(value),
                    "triggered by" => state.triggered_by = Some(value),
                    "part of" => state.part_of = Some(value),
                    _ => {}
                }
            }
            _ => {}
        }
    }
    state
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_loaded_system_job_reports_its_pid_and_argv() {
        let stdout = "STADO_LABEL_DOMAIN\tsystem\n\
             STADO_LABEL_FIELD\tpid\t57572\n\
             STADO_LABEL_FIELD\tstate\trunning\n\
             STADO_LABEL_FIELD\tpath\t/Library/LaunchDaemons/x.plist\n\
             STADO_LABEL_FIELD\targuments\t/u/.stado/bin/stado agent --target mini\n\
             STADO_LABEL_DONE\tyes\n";
        let state = parse_label_print("mini", "x", stdout);
        assert!(state.loaded());
        assert_eq!(state.domain.as_deref(), Some("system"));
        assert_eq!(state.pid.as_deref(), Some("57572"));
        assert_eq!(
            state.runs(),
            Some("/u/.stado/bin/stado agent --target mini")
        );
    }

    #[test]
    fn a_label_launchd_does_not_hold_is_not_loaded() {
        let state = parse_label_print("mini", "x", "STADO_LABEL_DONE\tno\n");
        assert!(!state.loaded());
        assert!(state.pid.is_none());
    }

    #[test]
    fn the_bare_program_is_used_only_when_there_is_no_argv() {
        let stdout = "STADO_LABEL_DOMAIN\tsystem\n\
             STADO_LABEL_FIELD\tprogram\t/u/.stado/bin/stado\n\
             STADO_LABEL_DONE\tyes\n";
        let state = parse_label_print("mini", "x", stdout);
        assert_eq!(state.runs(), Some("/u/.stado/bin/stado"));
    }

    #[test]
    fn an_environment_line_is_never_echoed() {
        // The remote filter takes a fixed key list; anything else must not be
        // parsed into the report even if a host somehow emitted it.
        let stdout = "STADO_LABEL_DOMAIN\tsystem\n\
             STADO_LABEL_FIELD\tSKARBIEC_TOKEN\tsecret-value\n\
             STADO_LABEL_DONE\tyes\n";
        let state = parse_label_print("mini", "x", stdout);
        let rendered = state.to_json().to_string();
        assert!(!rendered.contains("secret-value"));
    }

    #[test]
    fn the_remote_program_asks_only_for_named_scalars() {
        assert!(LABEL_PRINT_SCRIPT.contains("key == \\\"pid\\\""));
        assert!(LABEL_PRINT_SCRIPT.contains("/bin/launchctl print"));
        // Read-only: nothing in this program may act on the job.
        for verb in [
            "bootout",
            "bootstrap",
            "kickstart",
            "kill",
            "unload",
            "load",
        ] {
            assert!(!LABEL_PRINT_SCRIPT.contains(verb), "{verb} must not appear");
        }
    }

    #[test]
    fn an_unsupported_init_system_says_so() {
        let state = parse_label_print("box", "x", "STADO_LABEL_UNSUPPORTED\tFreeBSD\n");
        assert_eq!(state.unsupported.as_deref(), Some("FreeBSD"));
        assert!(!state.loaded());
    }
}
