//! `stado service label-print` — ask the host init system what it holds under
//! one named unit identity.
//!
//! Enumeration cannot find a loaded unit whose file was deleted, and a unit
//! file cannot say which environment or executable image launchd already
//! loaded. On launchd this reports fixed state fields, the five non-secret
//! storage-routing variables required by recovery, and the running process
//! start, executable path, and digest. It never emits the rest of launchd's
//! environment. A bounded exact-label event tail supplies recent spawn/exit
//! context. Systemd reports the matching fixed properties.
//!
//! It signals nothing, loads nothing and stops nothing. `service bootout` or
//! `service remove` are the commands that act; this one only answers.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

use super::service::{validate_unit_id, BootoutScope};
use super::{host_channel, shlex_quote, DeployError, Runner};
use crate::targets::ComputeTarget;

/// Read-only init-system query. Only named scalar properties, five explicitly
/// non-secret routing variables, one internally consistent running-image
/// identity, and a bounded exact-label event tail leave the host.
const LABEL_PRINT_SCRIPT: &str = "set -u
os=$(/usr/bin/uname -s)
label=@LABEL@
predicate=@PREDICATE@
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
      key == \"pid\" || key == \"state\" || key == \"last exit code\" || key == \"runs\" || key == \"path\" || key == \"stdout path\" || key == \"stderr path\" {
        value=$2
        sub(/^[ \\t]+/, \"\", value); sub(/[ \\t]+$/, \"\", value)
        printf \"STADO_LABEL_FIELD\\t%s\\t%s\\n\", key, value
      }'
    printf '%s\\n' \"$block\" | /usr/bin/awk '
      /^[ \\t]*program[ \\t]*=/ { line=$0; sub(/^[^=]*=[ \\t]*/, \"\", line); printf \"STADO_LABEL_FIELD\\tprogram\\t%s\\n\", line }
      /^[ \\t]*arguments[ \\t]*=[ \\t]*\\{/ { collecting=1; argv=\"\"; next }
      collecting && /^[ \\t]*\\}/ { collecting=0; sub(/^ /, \"\", argv); printf \"STADO_LABEL_FIELD\\targuments\\t%s\\n\", argv; next }
      collecting { line=$0; sub(/^[ \\t]+/, \"\", line); sub(/[ \\t]+$/, \"\", line); if (line != \"\") argv = argv \" \" line }'
    printf '%s\\n' \"$block\" | /usr/bin/awk '
      /^[ \\t]*environment[ \\t]*=[ \\t]*\\{/ { collecting=1; next }
      collecting && /^[ \\t]*\\}/ { collecting=0; next }
      collecting {
        line=$0
        sub(/^[ \\t]+/, \"\", line); sub(/[ \\t]+$/, \"\", line)
        split(line, pair, /[ \\t]+=>[ \\t]+/)
        key=pair[1]
        if (key == \"WC_STORAGE_BACKEND\" || key == \"WC_LOCAL_STORAGE_PATH\" ||
            key == \"WC_BACKUP_STORAGE_BACKEND\" ||
            key == \"WC_BACKUP_LOCAL_STORAGE_PATH\" || key == \"STADO_CONFIG\") {
          value=line
          sub(/^[^=]*=>[ \\t]*/, \"\", value)
          gsub(/\\t/, \" \", value)
          printf \"STADO_LABEL_ENV\\t%s\\t%s\\n\", key, value
        }
      }'
    launch_pid=$(printf '%s\\n' \"$block\" | /usr/bin/awk -F' = ' '$1 ~ /^[ \\t]*pid$/ { print $2; exit }')
    if [ -n \"$launch_pid\" ] && [ -x /usr/sbin/lsof ]; then
      process_start=$(/bin/ps -p \"$launch_pid\" -o lstart= 2>/dev/null |
        /usr/bin/awk '{$1=$1; print; exit}')
      mapping=$(/usr/sbin/lsof -a -p \"$launch_pid\" -d txt -F pDsikn 2>/dev/null |
        /usr/bin/awk '
          substr($0,1,1) == \"p\" { if (seen) exit; seen=1 }
          seen && substr($0,1,1) ~ /[Dsin]/ { print }
          seen && substr($0,1,1) == \"n\" { exit }')
      image=$(printf '%s\\n' \"$mapping\" |
        /usr/bin/awk 'substr($0,1,1) == \"n\" { print substr($0,2); exit }')
      mapped_device=$(printf '%s\\n' \"$mapping\" |
        /usr/bin/awk 'substr($0,1,1) == \"D\" { print substr($0,2); exit }')
      mapped_inode=$(printf '%s\\n' \"$mapping\" |
        /usr/bin/awk 'substr($0,1,1) == \"i\" { print substr($0,2); exit }')
      identity_error='process image could not be opened'
      image_digest=''
      opened_device=''
      opened_inode=''
      if [ -n \"$image\" ] && [ -n \"$mapped_device\" ] && [ -n \"$mapped_inode\" ] &&
         exec 9<\"$image\"; then
        opened_identity=$(/usr/bin/stat -f '%d:%i' <&9 2>/dev/null || true)
        opened_device=${opened_identity%%:*}
        opened_inode=${opened_identity#*:}
        mapped_device_decimal=$((mapped_device))
        if [ -n \"$opened_device\" ] && [ -n \"$opened_inode\" ] &&
           [ \"$opened_device\" -eq \"$mapped_device_decimal\" ] &&
           [ \"$opened_inode\" -eq \"$mapped_inode\" ]; then
          image_digest=$(/usr/bin/openssl dgst -sha256 -r <&9 2>/dev/null)
          image_digest=${image_digest%% *}
          after_identity=$(/usr/bin/stat -f '%d:%i' <&9 2>/dev/null || true)
          after_device=${after_identity%%:*}
          after_inode=${after_identity#*:}
          if [ \"$after_device\" != \"$opened_device\" ] ||
             [ \"$after_inode\" != \"$opened_inode\" ]; then
            image_digest=''
            identity_error='opened image changed during hashing'
          fi
        else
          identity_error='mapped image no longer names the opened inode'
        fi
        exec 9<&-
      fi
      current=$(/bin/launchctl print \"$domain/$label\" 2>/dev/null || true)
      current_pid=$(printf '%s\\n' \"$current\" |
        /usr/bin/awk -F' = ' '$1 ~ /^[ \\t]*pid$/ { print $2; exit }')
      current_start=$(/bin/ps -p \"$current_pid\" -o lstart= 2>/dev/null |
        /usr/bin/awk '{$1=$1; print; exit}')
      current_mapping=$(/usr/sbin/lsof -a -p \"$current_pid\" -d txt -F pDsikn 2>/dev/null |
        /usr/bin/awk '
          substr($0,1,1) == \"p\" { if (seen) exit; seen=1 }
          seen && substr($0,1,1) ~ /[Dsin]/ { print }
          seen && substr($0,1,1) == \"n\" { exit }')
      case \"$image_digest\" in
        [0-9a-f][0-9a-f]*)
          if [ \"$current_pid\" = \"$launch_pid\" ] &&
             [ -n \"$process_start\" ] && [ \"$current_start\" = \"$process_start\" ] &&
             [ -n \"$mapping\" ] && [ \"$current_mapping\" = \"$mapping\" ]; then
            printf 'STADO_LABEL_FIELD\\tprocess start\\t%s\\n' \"$process_start\"
            printf 'STADO_LABEL_FIELD\\tprocess executable\\t%s\\n' \"$image\"
            printf 'STADO_LABEL_FIELD\\tprocess device\\t%s\\n' \"$opened_device\"
            printf 'STADO_LABEL_FIELD\\tprocess inode\\t%s\\n' \"$opened_inode\"
            printf 'STADO_LABEL_FIELD\\tprocess sha256\\t%s\\n' \"$image_digest\"
          else
            printf 'STADO_LABEL_IDENTITY_UNAVAILABLE\\tprocess changed during identity capture\\n'
          fi
          ;;
        *) printf 'STADO_LABEL_IDENTITY_UNAVAILABLE\\t%s\\n' \"$identity_error\" ;;
      esac
    elif [ -n \"$launch_pid\" ]; then
      printf 'STADO_LABEL_IDENTITY_UNAVAILABLE\\tlsof is unavailable\\n'
    fi
    if [ -x /usr/bin/log ]; then
      {
        /usr/bin/log show --last 1h --style compact --predicate \"$predicate\" 2>&1
        printf 'STADO_LABEL_EVENT_EXIT\\t%s\\n' \"$?\"
      } |
        STADO_LABEL=\"$label\" STADO_QUALIFIED=\"$domain/$label\" LC_ALL=C /usr/bin/awk '
          function complete_identity_field(line, identity) {
            return index(line, \"(\" identity \")\") > 0 ||
                   index(line, \"[\" identity \"]\") > 0 ||
                   index(line, \"[\" identity \":]\") > 0 ||
                   index(line, \"[\" identity \" [\") > 0
          }
          BEGIN {
            label=ENVIRON[\"STADO_LABEL\"]
            qualified=ENVIRON[\"STADO_QUALIFIED\"]
          }
          index($0, \"STADO_LABEL_EVENT_EXIT\\t\") == 1 {
            status=substr($0, length(\"STADO_LABEL_EVENT_EXIT\\t\") + 1)
            saw_status=1
            next
          }
          {
            detail=substr($0, 1, 512)
            if (complete_identity_field($0, qualified) ||
                complete_identity_field($0, label)) {
              count++
              events[(count - 1) % 12]=substr($0, 1, 2048)
            }
          }
          END {
            if (!saw_status) {
              printf \"STADO_LABEL_EVENT_STATUS\\terror: log reader returned no status\\n\"
            } else if (status != 0) {
              gsub(/\\t/, \" \", detail)
              printf \"STADO_LABEL_EVENT_STATUS\\terror %s: %s\\n\", status, detail
            } else {
              first=count > 12 ? count - 11 : 1
              for (number=first; number <= count; number++) {
                event=events[(number - 1) % 12]
                gsub(/\\t/, \" \", event)
                printf \"STADO_LABEL_EVENT\\t%s\\n\", event
              }
              printf \"STADO_LABEL_EVENT_STATUS\\tok\\n\"
            }
          }'
    else
      printf 'STADO_LABEL_EVENT_STATUS\\tunavailable: /usr/bin/log is absent\\n'
    fi
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
    main_pid=$(printf '%s\\n' \"$block\" | /usr/bin/awk -F= '$1 == \"MainPID\" { print $2; exit }')
    case \"$main_pid\" in
      ''|0|*[!0-9]*) ;;
      *)
        process_start=$(/usr/bin/ps -p \"$main_pid\" -o lstart= 2>/dev/null |
          /usr/bin/awk '{$1=$1; print; exit}')
        image=$(/usr/bin/readlink \"/proc/$main_pid/exe\" 2>/dev/null || true)
        mapped_device=$(/usr/bin/stat -Lc '%d' \"/proc/$main_pid/exe\" 2>/dev/null || true)
        mapped_inode=$(/usr/bin/stat -Lc '%i' \"/proc/$main_pid/exe\" 2>/dev/null || true)
        identity_error='process image could not be opened'
        image_digest=''
        opened_device=''
        opened_inode=''
        if [ -n \"$image\" ] && [ -n \"$mapped_device\" ] && [ -n \"$mapped_inode\" ] &&
           exec 9<\"/proc/$main_pid/exe\"; then
          opened_device=$(/usr/bin/stat -Lc '%d' /dev/fd/9 2>/dev/null || true)
          opened_inode=$(/usr/bin/stat -Lc '%i' /dev/fd/9 2>/dev/null || true)
          if [ \"$opened_device\" = \"$mapped_device\" ] &&
             [ \"$opened_inode\" = \"$mapped_inode\" ]; then
            image_digest=$(/usr/bin/openssl dgst -sha256 -r /dev/fd/9 2>/dev/null)
            image_digest=${image_digest%% *}
            after_device=$(/usr/bin/stat -Lc '%d' /dev/fd/9 2>/dev/null || true)
            after_inode=$(/usr/bin/stat -Lc '%i' /dev/fd/9 2>/dev/null || true)
            if [ \"$after_device\" != \"$opened_device\" ] ||
               [ \"$after_inode\" != \"$opened_inode\" ]; then
              image_digest=''
              identity_error='opened image changed during hashing'
            fi
          else
            identity_error='mapped image no longer names the opened inode'
          fi
          exec 9<&-
        fi
        if [ \"$domain\" = system ]; then
          if [ \"$uid\" = 0 ]; then
            current_pid=$(/usr/bin/systemctl show \"$label\" --property=MainPID --value 2>/dev/null || true)
          else
            current_pid=$(/usr/bin/sudo -n /usr/bin/systemctl show \"$label\" --property=MainPID --value 2>/dev/null || true)
          fi
        else
          current_pid=$(/usr/bin/env XDG_RUNTIME_DIR=\"$runtime\" DBUS_SESSION_BUS_ADDRESS=\"unix:path=$runtime/bus\" /usr/bin/systemctl --user show \"$label\" --property=MainPID --value 2>/dev/null || true)
        fi
        current_start=$(/usr/bin/ps -p \"$current_pid\" -o lstart= 2>/dev/null |
          /usr/bin/awk '{$1=$1; print; exit}')
        current_image=$(/usr/bin/readlink \"/proc/$current_pid/exe\" 2>/dev/null || true)
        current_device=$(/usr/bin/stat -Lc '%d' \"/proc/$current_pid/exe\" 2>/dev/null || true)
        current_inode=$(/usr/bin/stat -Lc '%i' \"/proc/$current_pid/exe\" 2>/dev/null || true)
        case \"$image_digest\" in
          [0-9a-f][0-9a-f]*)
            if [ \"$current_pid\" = \"$main_pid\" ] &&
               [ -n \"$process_start\" ] && [ \"$current_start\" = \"$process_start\" ] &&
               [ -n \"$image\" ] && [ \"$current_image\" = \"$image\" ] &&
               [ \"$current_device\" = \"$opened_device\" ] &&
               [ \"$current_inode\" = \"$opened_inode\" ]; then
              printf 'STADO_LABEL_FIELD\\tprocess start\\t%s\\n' \"$process_start\"
              printf 'STADO_LABEL_FIELD\\tprocess executable\\t%s\\n' \"$image\"
              printf 'STADO_LABEL_FIELD\\tprocess device\\t%s\\n' \"$opened_device\"
              printf 'STADO_LABEL_FIELD\\tprocess inode\\t%s\\n' \"$opened_inode\"
              printf 'STADO_LABEL_FIELD\\tprocess sha256\\t%s\\n' \"$image_digest\"
            else
              printf 'STADO_LABEL_IDENTITY_UNAVAILABLE\\tprocess changed during identity capture\\n'
            fi
            ;;
          *) printf 'STADO_LABEL_IDENTITY_UNAVAILABLE\\t%s\\n' \"$identity_error\" ;;
        esac
        ;;
    esac
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
    pub stdout_path: Option<String>,
    /// Exact non-secret routing values from launchd's loaded environment.
    pub loaded_environment: BTreeMap<String, String>,
    /// Executable image currently mapped by the launchd pid, when lsof can
    /// resolve it.
    pub process_executable: Option<String>,
    /// SHA-256 of `process_executable` read while that pid is current.
    /// Device and inode of the opened executable whose bytes were hashed,
    /// equal to the init system pid's mapped executable before and after.
    pub process_device: Option<u64>,
    pub process_inode: Option<u64>,
    pub process_sha256: Option<String>,
    /// Init-system process start spelling observed for `pid`.
    pub process_started_at: Option<String>,
    /// Why a pid did not yield one consistent process tuple.
    pub process_identity_unavailable: Option<String>,
    pub stderr_path: Option<String>,
    /// At most twelve launchd events from the preceding hour whose message
    /// names this exact validated label. Empty on Linux and unsupported hosts.
    pub recent_events: Vec<String>,
    /// Whether launchd's bounded event read succeeded. The failure detail is
    /// capped remotely and never substitutes for an empty successful result.
    pub event_read_status: Option<String>,
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
        let mut report = json!({
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
            "loaded_environment": self.loaded_environment,
            "process_executable": self.process_executable,
            "process_device": self.process_device,
            "process_inode": self.process_inode,
            "process_started_at": self.process_started_at,
            "process_identity_unavailable": self.process_identity_unavailable,
            "process_sha256": self.process_sha256,
            "unit_file_state": self.unit_file_state,
            "restart": self.restart,
            "triggers": self.triggers,
            "triggered_by": self.triggered_by,
            "part_of": self.part_of,
            "unsupported": self.unsupported,
        });
        if self.event_read_status.is_some() {
            let fields = report
                .as_object_mut()
                .expect("the label report is always a JSON object");
            fields.insert("stdout_path".to_string(), json!(self.stdout_path));
            fields.insert("stderr_path".to_string(), json!(self.stderr_path));
            fields.insert("recent_events".to_string(), json!(self.recent_events));
            fields.insert(
                "event_read_status".to_string(),
                json!(self.event_read_status),
            );
        }
        report
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
    let predicate = format!(
        "process == \"launchd\" AND eventMessage CONTAINS \"{}\"",
        label.replace('\\', "\\\\").replace('"', "\\\"")
    );
    let script = LABEL_PRINT_SCRIPT
        .replace("@LABEL@", &shlex_quote(label))
        .replace("@PREDICATE@", &shlex_quote(&predicate))
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
                    "pid" => {
                        state.pid = value
                            .parse::<u32>()
                            .ok()
                            .filter(|pid| *pid != 0)
                            .map(|pid| pid.to_string());
                    }
                    "state" => state.state = Some(value),
                    "last exit code" => state.last_exit_code = Some(value),
                    "runs" => state.runs = Some(value),
                    "path" => state.path = Some(value),
                    "program" => state.program = Some(value),
                    "arguments" => state.arguments = Some(value),
                    "process executable" => state.process_executable = Some(value),
                    "process device" => state.process_device = value.parse().ok(),
                    "process inode" => state.process_inode = value.parse().ok(),
                    "process start" => state.process_started_at = Some(value),
                    "process sha256" => state.process_sha256 = Some(value),
                    "stdout path" => state.stdout_path = Some(value),
                    "stderr path" => state.stderr_path = Some(value),
                    "unit file state" => state.unit_file_state = Some(value),
                    "restart" => state.restart = Some(value),
                    "triggers" => state.triggers = Some(value),
                    "triggered by" => state.triggered_by = Some(value),
                    "part of" => state.part_of = Some(value),
                    _ => {}
                }
            }
            ["STADO_LABEL_ENV", key, value] => {
                let key = (*key).trim();
                let value = (*value).trim();
                if matches!(
                    key,
                    "WC_STORAGE_BACKEND"
                        | "WC_LOCAL_STORAGE_PATH"
                        | "WC_BACKUP_STORAGE_BACKEND"
                        | "WC_BACKUP_LOCAL_STORAGE_PATH"
                        | "STADO_CONFIG"
                ) && !value.is_empty()
                {
                    state
                        .loaded_environment
                        .insert(key.to_string(), value.to_string());
                }
            }
            ["STADO_LABEL_IDENTITY_UNAVAILABLE", reason] => {
                let reason = (*reason).trim();
                if !reason.is_empty() {
                    state.process_identity_unavailable = Some(reason.to_string());
                }
            }
            ["STADO_LABEL_EVENT", event] => {
                let event = (*event).trim();
                if !event.is_empty() {
                    state.recent_events.push(event.to_string());
                }
            }
            ["STADO_LABEL_EVENT_STATUS", status] => {
                let status = (*status).trim();
                if !status.is_empty() {
                    state.event_read_status = Some(status.to_string());
                }
            }
            _ => {}
        }
    }
    let identity_fields = usize::from(state.process_executable.is_some())
        + usize::from(state.process_started_at.is_some())
        + usize::from(state.process_device.is_some())
        + usize::from(state.process_inode.is_some())
        + usize::from(state.process_sha256.is_some());
    let digest_valid = state.process_sha256.as_deref().is_none_or(|digest| {
        digest.len() == 64
            && digest
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    });
    if identity_fields != 0
        && (identity_fields != 5 || state.process_inode == Some(0) || !digest_valid)
    {
        state.process_executable = None;
        state.process_started_at = None;
        state.process_device = None;
        state.process_inode = None;
        state.process_sha256 = None;
        state.process_identity_unavailable =
            Some("host returned an incomplete process identity tuple".to_string());
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
