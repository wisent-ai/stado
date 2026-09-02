//! `stado service label-print` — ask launchd what it holds under one named
//! label, in one named domain, and print only the facts.
//!
//! NO Python original. This closes the last gap in the 2026-09-01
//! charless-mac-mini hunt (issue #286). An **undeclared** `stado agent` kept
//! reappearing there, and every ownership reader in this binary answered that
//! no launchd label held it:
//!
//! - the reap keep-set probes `launchctl print` only for labels the REGISTRY
//!   DECLARES, so an undeclared job is never asked about;
//! - `launchctl list` cannot print the system domain at all;
//! - [`super::service::loaded_units`] unions that listing with the unit files
//!   in the three directories this fleet installs into, so a job whose plist
//!   has been DELETED while the job stays loaded is in neither half.
//!
//! The owner turned out to be a system-domain job under
//! `com.wisent.compute.service.com.wisent.compute.service.stado-agent-mini` —
//! the fleet's own prefix minted onto a name that already carried it — with no
//! file left on disk. Nothing shipped could name it, because every reader
//! enumerated a population and this label was outside all of them. The reader
//! that works does not enumerate: it asks about the label the operator names.
//!
//! So the contract is deliberately the inverse of `list --undeclared`:
//!
//! - **the operator supplies the label**, and no scan decides which labels are
//!   askable. That is the whole point — a label nothing enumerates is exactly
//!   the label worth asking about;
//! - `launchctl print <domain>/<label>` is read-only and needs no privilege on
//!   Darwin, which PR #285 verified when the reap keep-set started using it;
//! - only a small fixed set of scalar lines is taken out of the output —
//!   `pid`, `state`, `last exit code`, `runs`, `path`, `program` — because
//!   `launchctl print` also dumps the job's full environment, and this fleet's
//!   units carry tokens there. Reporting the whole block would be a diagnostic
//!   command that prints credentials into terminals and logs. Everything not
//!   on that list never leaves the host.
//!
//! It signals nothing, loads nothing, boots nothing out. `service bootout` is
//! the command that acts; this one only answers.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::service::{quote_unit_path, validate_unit_id, BootoutScope};
use super::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Read-only: `launchctl print` in the domains the scope allows, and nothing
/// else. It starts nothing, stops nothing, signals nothing and needs no sudo.
///
/// The domain order matches [`super::service::BOOTOUT_SCRIPT`]'s, so
/// `label-print` and `bootout` can never disagree about which job an operator
/// is looking at.
///
/// Only the named scalar lines are echoed. `launchctl print` emits the job's
/// entire environment block; a unit on this fleet carries bearer tokens in it,
/// so the filter is a safety property and not tidiness.
const LABEL_PRINT_SCRIPT: &str = "set -u
if [ \"$(/usr/bin/uname -s)\" != Darwin ]; then
  printf 'STADO_LABEL_UNSUPPORTED\\t%s\\n' \"$(/usr/bin/uname -s)\"
  exit 0
fi
label=@LABEL@
scope=@SCOPE@
uid=$(/usr/bin/id -u)
case \"$scope\" in
  system) domains='system' ;;
  user)   domains=\"user/$uid gui/$uid\" ;;
  *)      domains=\"system user/$uid gui/$uid\" ;;
esac
found=no
for domain in $domains; do
  block=$(/bin/launchctl print \"$domain/$label\" 2>/dev/null) || continue
  if [ -z \"$block\" ]; then continue; fi
  found=yes
  printf 'STADO_LABEL_DOMAIN\\t%s\\n' \"$domain\"
  # A fixed list of scalars, never the whole block: `launchctl print` dumps the
  # job's environment and this fleet's units keep tokens in it.
  printf '%s\\n' \"$block\" | /usr/bin/awk -F' = ' '
    { key=$1; sub(/^[ \\t]+/, \"\", key); sub(/[ \\t]+$/, \"\", key) }
    key == \"pid\" || key == \"state\" || key == \"last exit code\" || key == \"runs\" || key == \"path\" {
      value=$2
      sub(/^[ \\t]+/, \"\", value); sub(/[ \\t]+$/, \"\", value)
      printf \"STADO_LABEL_FIELD\\t%s\\t%s\\n\", key, value
    }'
  # The program and its arguments, which live in their own indented blocks
  # rather than on ` = ` lines.
  printf '%s\\n' \"$block\" | /usr/bin/awk '
    /^[ \\t]*program[ \\t]*=/ { line=$0; sub(/^[^=]*=[ \\t]*/, \"\", line); printf \"STADO_LABEL_FIELD\\tprogram\\t%s\\n\", line }
    /^[ \\t]*arguments[ \\t]*=[ \\t]*\\{/ { collecting=1; argv=\"\"; next }
    collecting && /^[ \\t]*\\}/ { collecting=0; sub(/^ /, \"\", argv); printf \"STADO_LABEL_FIELD\\targuments\\t%s\\n\", argv; next }
    collecting { line=$0; sub(/^[ \\t]+/, \"\", line); sub(/[ \\t]+$/, \"\", line); if (line != \"\") argv = argv \" \" line }'
  break
done
printf 'STADO_LABEL_DONE\\t%s\\n' \"$found\"
";

/// What launchd holds under one label.
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
    /// The unit file launchd loaded the job from. A job whose plist has since
    /// been deleted still reports the path it was loaded from, which is how a
    /// file-less job gets named at all.
    pub path: Option<String>,
    pub program: Option<String>,
    pub arguments: Option<String>,
    /// Set when the host is not Darwin, naming the system it reported.
    pub unsupported: Option<String>,
}

impl LabelState {
    /// Did launchd answer for this label in any allowed domain?
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
            "unsupported": self.unsupported,
        })
    }

    /// `runs` in the launchd sense — how many times the job has been started.
    /// Named apart from [`Self::runs`] so the two cannot be confused.
    fn runs_field(&self) -> Option<&str> {
        self.runs.as_deref()
    }
}

/// Ask one host what it holds under one label.
///
/// Signals nothing. The only thing this can do to a host is read launchd.
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
    fn a_non_darwin_host_says_so() {
        let state = parse_label_print("box", "x", "STADO_LABEL_UNSUPPORTED\tLinux\n");
        assert_eq!(state.unsupported.as_deref(), Some("Linux"));
        assert!(!state.loaded());
    }
}
