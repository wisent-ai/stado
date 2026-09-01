//! Is the declared unit the process on its own port?
//!
//! NO Python original. This module exists because of what `service show`
//! answered on 2026-08-30. It reported `com.wisent.always-on.weles` as `runs`
//! while both pids the preceding restart had reported were already gone from
//! `ps` and the unit's stderr ended in `EADDRINUSE 127.0.0.1:58101`. The unit
//! was dead and the control plane called it healthy, which is why nobody
//! noticed for days.
//!
//! The reason is worth stating exactly, because it is a shape this fleet has
//! now met three times. `SHOW_BODY` says `runs` whenever the unit FILE exists:
//! it reads `ProgramArguments` out of the plist and reports what the unit
//! declares. That is a useful answer to a different question. It is a
//! declaration nobody checked against the world — the same defect as a forward
//! marker naming a port nothing served, and as an env key no reader ever read
//! back. [`super::host_inventory`] closed the first by reconciling markers
//! against live listeners and [`super::service_env_file`] closed the second by
//! reading a write back. This closes the third, one level further down: not
//! "is something listening there" but "is the thing listening there the
//! process this unit owns".
//!
//! Three properties are deliberate:
//!
//! 1. **Ownership is decided by launchd label, never by argv.** On
//!    charless-mac-mini the declared `com.wisent.always-on.weles` and the
//!    undeclared `com.wisent.weles-worker` execute the same program with the
//!    same argument vector — the Weles release deployer bootstraps the second
//!    one by design. [`super::service::stado_unit_pids`]-style argv matching
//!    would attribute the surviving process to whichever unit was asked about
//!    and answer `serving` for a unit that is down. So the question asked here
//!    is which launchd job holds the pid, resolved by walking the pid's parent
//!    chain until a pid appears in `launchctl list`.
//! 2. **An owner that cannot be read is [`OWNER_UNKNOWN`], never
//!    "undeclared".** An unprivileged `launchctl list` shows the caller's
//!    per-login domain and not the system domain, so a system LaunchDaemon's
//!    label is genuinely unresolvable over the approved channel. Reporting
//!    that as "no unit owns this port" would turn every working daemon into a
//!    finding.
//! 3. **A check that could not be performed is not a check that passed.** The
//!    verdict [`SERVING_UNKNOWN`] exists and exits non-zero, exactly as
//!    [`super::service_env_file`]'s `listeners_state` does, because "nothing
//!    is listening" and "nobody could look" are opposite findings that look
//!    identical in an empty list.
//!
//! The transport is [`host_channel::run_script`] and the listener read is the
//! same `lsof -nP -iTCP:<port> -sTCP:LISTEN` spelling
//! [`super::service::LISTENER_RESET_BODY`] already uses, so this command and
//! every other reader of "what is listening" cannot disagree.

use base64::engine::general_purpose::STANDARD;
use base64::Engine as _;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{host_channel, DeployError, Runner};
use crate::targets::ComputeTarget;

/// `status` for a report that came back whole.
pub const OK_STATUS: &str = "service_serving";

/// The owning launchd label was resolved from the pid's own parent chain.
pub const OWNER_RESOLVED: &str = "resolved";
/// No label in the readable domain claims this pid or any of its ancestors.
/// A system LaunchDaemon is invisible to an unprivileged `launchctl list`, so
/// this is never reported as "nothing owns it".
pub const OWNER_UNKNOWN: &str = "unknown";

/// The process holding this port belongs to the unit under test.
pub const PORT_SERVED_BY_UNIT: &str = "served_by_unit";
/// Something is listening and a DIFFERENT launchd job owns it. The finding
/// this module was written for.
pub const PORT_SERVED_BY_OTHER: &str = "served_by_other";
/// Nothing is listening on the port, and the socket table was really read.
pub const PORT_DEAD: &str = "dead";
/// Something is listening and whose job it is could not be established.
pub const PORT_OWNER_UNKNOWN: &str = "owner_unknown";
/// The socket table could not be read, so the port was not judged.
pub const PORT_UNKNOWN: &str = "unknown";

/// Every declared port is held by this unit's own process.
pub const SERVING_YES: &str = "serving";
/// At least one declared port is dead or held by another job.
pub const SERVING_NO: &str = "not_serving";
/// The question could not be answered.
pub const SERVING_UNKNOWN: &str = "unknown";

/// Listeners came from `lsof`.
pub const LISTENERS_READ: &str = "read";
/// Neither reader answered.
pub const LISTENERS_FAILED: &str = "failed";

/// How many parent links the owner walk follows before giving up. A launchd
/// job's own pid is the process or a near ancestor of it; eight is far past
/// any real launcher chain and bounds the walk on a hostile process tree.
pub const MAX_OWNER_DEPTH: u32 = 8;

/// The cap on ports one report judges.
pub const MAX_PORTS: usize = 32;

/// One process holding a declared port.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Holder {
    pub pid: String,
    /// The executable, as `ps -o comm=` prints it. Never the argument vector:
    /// a command line can carry a secret and this report does not need one.
    pub comm: String,
    /// The launchd label whose job this pid belongs to, empty when unresolved.
    pub owner: String,
    /// [`OWNER_RESOLVED`] or [`OWNER_UNKNOWN`].
    pub owner_state: String,
}

/// One declared port and who answers on it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortReport {
    pub port: u16,
    pub holders: Vec<Holder>,
}

/// Everything the remote script reported.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServingReport {
    pub unit: String,
    pub unit_path: String,
    /// `yes` when launchd knows the label in the readable domain.
    pub loaded: String,
    /// The pid launchd reports for the label, empty when it holds none.
    pub launchd_pid: String,
    /// [`LISTENERS_READ`] or [`LISTENERS_FAILED`].
    pub listeners_state: String,
    pub ports: Vec<PortReport>,
}

/// One port's verdict, and the sentence an operator acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortVerdict {
    pub port: u16,
    pub verdict: &'static str,
    pub holders: Vec<Holder>,
}

impl PortVerdict {
    /// Every holder that is not this unit, spelled for a table cell.
    pub fn holder_cell(&self) -> String {
        if self.holders.is_empty() {
            return "-".to_string();
        }
        self.holders
            .iter()
            .map(|holder| {
                let owner = if holder.owner_state == OWNER_RESOLVED && !holder.owner.is_empty() {
                    holder.owner.clone()
                } else {
                    "owner unknown".to_string()
                };
                format!("pid {} {} ({owner})", holder.pid, holder.comm)
            })
            .collect::<Vec<String>>()
            .join(", ")
    }
}

/// Whether this pid, or the job that owns it, is the unit under test.
///
/// The label is authoritative. The launchd pid is accepted as well because a
/// job whose program `exec`s in place keeps the pid launchd recorded, and on
/// that path the label lookup and the pid agree; where they disagree the label
/// wins, which is what stops one of two units running identical argv from
/// claiming the other's process.
fn belongs_to_unit(holder: &Holder, unit: &str, launchd_pid: &str) -> bool {
    if holder.owner_state == OWNER_RESOLVED {
        return holder.owner == unit;
    }
    !launchd_pid.is_empty() && holder.pid == launchd_pid
}

/// Judge every port the caller declared.
pub fn port_verdicts(report: &ServingReport) -> Vec<PortVerdict> {
    report
        .ports
        .iter()
        .map(|port| {
            let verdict = if report.listeners_state != LISTENERS_READ {
                PORT_UNKNOWN
            } else if port.holders.is_empty() {
                PORT_DEAD
            } else if port
                .holders
                .iter()
                .any(|holder| belongs_to_unit(holder, &report.unit, &report.launchd_pid))
            {
                PORT_SERVED_BY_UNIT
            } else if port
                .holders
                .iter()
                .all(|holder| holder.owner_state == OWNER_UNKNOWN)
            {
                PORT_OWNER_UNKNOWN
            } else {
                PORT_SERVED_BY_OTHER
            };
            PortVerdict {
                port: port.port,
                verdict,
                holders: port.holders.clone(),
            }
        })
        .collect()
}

/// The one word for the whole unit.
///
/// `serving` requires every declared port to be held by this unit's own
/// process. Anything unreadable is `unknown` rather than either of the other
/// two, and a port held by another job is `not_serving` — the case that used
/// to read as `runs`.
pub fn verdict(report: &ServingReport, ports: &[PortVerdict]) -> &'static str {
    if report.listeners_state != LISTENERS_READ {
        return SERVING_UNKNOWN;
    }
    if ports.is_empty() {
        return SERVING_UNKNOWN;
    }
    if ports
        .iter()
        .any(|port| matches!(port.verdict, PORT_SERVED_BY_OTHER | PORT_DEAD))
    {
        return SERVING_NO;
    }
    if ports.iter().any(|port| port.verdict != PORT_SERVED_BY_UNIT) {
        return SERVING_UNKNOWN;
    }
    SERVING_YES
}

/// Why this unit is not serving, in the operator's words, or `None`.
pub fn failure(host: &str, report: &ServingReport, ports: &[PortVerdict]) -> Option<String> {
    match verdict(report, ports) {
        SERVING_YES => None,
        SERVING_UNKNOWN if report.listeners_state != LISTENERS_READ => Some(format!(
            "{host}: the socket table could not be read, so no declared port below was judged"
        )),
        SERVING_UNKNOWN if ports.is_empty() => Some(format!(
            "{host}: {} declares no loopback port, so whether it serves cannot be decided here",
            report.unit
        )),
        SERVING_UNKNOWN => Some(format!(
            "{host}: something answers on {}'s declared port(s) and which launchd job owns it \
             could not be established over this channel",
            report.unit
        )),
        _ => {
            let taken: Vec<String> = ports
                .iter()
                .filter(|port| port.verdict == PORT_SERVED_BY_OTHER)
                .map(|port| format!("{} is held by {}", port.port, port.holder_cell()))
                .collect();
            let dead: Vec<String> = ports
                .iter()
                .filter(|port| port.verdict == PORT_DEAD)
                .map(|port| port.port.to_string())
                .collect();
            let mut said = format!("{host}: {} is not serving", report.unit);
            if !taken.is_empty() {
                said.push_str(&format!(" — {}", taken.join("; ")));
            }
            if !dead.is_empty() {
                said.push_str(&format!(" — nothing is listening on {}", dead.join(", ")));
            }
            Some(said)
        }
    }
}

/// The remote program.
///
/// One launchd-domain scan is read once and reused for every owner walk: the
/// question "which job owns this pid" is asked per holder, and forking a
/// `launchctl` per holder would make the answer depend on how many processes
/// happened to hold the port. `launchctl list` sees only the caller's login
/// domain; the service manager on the always-on Mac owns jobs in `gui/<uid>`,
/// `user/<uid>`, and `system`. Reading the printable service tables is what
/// turns the pid that `service list --unowned` already proves is owned into the
/// exact label `service serving` needs.
const REMOTE_SERVING_BODY: &str = r##"
decode_flag=-D
if [ "$os" = "Linux" ]; then decode_flag=--decode; fi
ports_raw=$(printf '%s' '@PORTS_B64@' | /usr/bin/base64 "$decode_flag")

stado_launchd_state

# Read every printable launchd domain once. Each row is `pid<TAB>label`; a job
# whose pid is zero has no process to own and is deliberately omitted.
lc_table=''
if [ "$os" = "Darwin" ]; then
  uid=$(/usr/bin/id -u)
  for lc_domain in "gui/$uid" "user/$uid" system; do
    lc_rows=$(/bin/launchctl print "$lc_domain" 2>/dev/null | /usr/bin/awk '
      /services = \{/ { inside = 1; next }
      inside && /^[[:space:]]*\}/ { inside = 0 }
      inside && $1 ~ /^[1-9][0-9]*$/ && NF >= 3 {
        print $1 "\t" $3
      }
    ')
    lc_table="$lc_table${lc_table:+
}$lc_rows"
  done
fi

# The launchd job that owns a pid: the first pid on its own parent chain that
# one of the printable domains claims. Walking the chain is required, not
# defensive — a launcher script is the job and the server it starts is the
# child that holds the socket, so the listening pid is usually NOT the pid
# launchd recorded.
owner_label=''
owner_state=''
resolve_owner() {
  ro_pid="$1"
  ro_depth=0
  owner_label=''
  owner_state='unknown'
  while [ -n "$ro_pid" ] && [ "$ro_pid" != "1" ] && [ "$ro_depth" -lt @MAX_DEPTH@ ]; do
    ro_found=$(printf '%s\n' "$lc_table" | /usr/bin/awk -F'\t' -v P="$ro_pid" '$1 == P { print $2; exit }')
    if [ -n "$ro_found" ]; then
      owner_label="$ro_found"
      owner_state='resolved'
      return 0
    fi
    ro_pid=$(/bin/ps -p "$ro_pid" -o ppid= 2>/dev/null | /usr/bin/tr -d ' ')
    ro_depth=$((ro_depth + 1))
  done
  return 0
}

# Printable ASCII only, minus the two bytes a JSON string cannot carry raw.
# A label and a program name are host text and this report must not be
# breakable by either.
jsonsafe() {
  printf '%s' "$1" | /usr/bin/tr -c ' -~' '?' | /usr/bin/tr '"\\' '??'
}

listeners_state='read'
if ! /usr/sbin/lsof -nP -iTCP -sTCP:LISTEN >/dev/null 2>&1; then
  listeners_state='failed'
fi

ports_json=''
for port in $ports_raw; do
  case "$port" in ''|*[!0-9]*) continue ;; esac
  holders_json=''
  if [ "$listeners_state" = read ]; then
    for hpid in $(/usr/sbin/lsof -nP -tiTCP:"$port" -sTCP:LISTEN 2>/dev/null); do
      case "$hpid" in ''|*[!0-9]*) continue ;; esac
      hcomm=$(/bin/ps -p "$hpid" -o comm= 2>/dev/null | /usr/bin/sed 's/^ *//;s/ *$//')
      resolve_owner "$hpid"
      holders_json="$holders_json${holders_json:+,}{\"pid\":\"$hpid\",\"comm\":\"$(jsonsafe "$hcomm")\",\"owner\":\"$(jsonsafe "$owner_label")\",\"owner_state\":\"$owner_state\"}"
    done
  fi
  ports_json="$ports_json${ports_json:+,}{\"port\":$port,\"holders\":[$holders_json]}"
done

printf '{"unit":"%s","unit_path":"%s","loaded":"%s","launchd_pid":"%s","listeners_state":"%s","ports":[%s]}\n' \
  "$(jsonsafe "$unit")" "$(jsonsafe "$unit_path")" "$pc_loaded" "$(jsonsafe "$pc_pid")" \
  "$listeners_state" "$ports_json"
"##;

/// The remote program for one unit and one set of ports.
///
/// The ports travel base64-encoded inside the script body, never in an
/// argument vector, for the same reason every other reader here encodes its
/// operands.
pub fn remote_serving_script(ports: &[u16]) -> String {
    let list = ports
        .iter()
        .take(MAX_PORTS)
        .map(u16::to_string)
        .collect::<Vec<String>>()
        .join(" ");
    REMOTE_SERVING_BODY
        .replace("@PORTS_B64@", &STANDARD.encode(list.as_bytes()))
        .replace("@MAX_DEPTH@", &MAX_OWNER_DEPTH.to_string())
}

/// Parse the script's one line of JSON.
pub fn parse_serving(stdout: &str) -> Result<ServingReport, DeployError> {
    let payload = stdout
        .lines()
        .rev()
        .map(str::trim)
        .find(|line| line.starts_with('{'))
        .ok_or_else(|| DeployError("serving script produced no JSON report".to_string()))?;
    serde_json::from_str(payload).map_err(|error| {
        DeployError(format!(
            "serving script did not return the expected JSON: {error}"
        ))
    })
}

/// The report as `--json`, in [`super::host_inventory`]'s report shape.
pub fn to_report(
    target: &ComputeTarget,
    report: &ServingReport,
    ports: &[PortVerdict],
    declared_owners: &dyn Fn(&str) -> bool,
) -> Map<String, Value> {
    let mut object = Map::new();
    object.insert("host".to_string(), json!(target.name));
    object.insert("unit".to_string(), json!(report.unit));
    object.insert("status".to_string(), json!(OK_STATUS));
    object.insert("loaded".to_string(), json!(report.loaded));
    object.insert("launchd_pid".to_string(), json!(report.launchd_pid));
    object.insert("listeners_state".to_string(), json!(report.listeners_state));
    object.insert("serving".to_string(), json!(verdict(report, ports)));
    object.insert(
        "ports".to_string(),
        Value::Array(
            ports
                .iter()
                .map(|port| {
                    json!({
                        "port": port.port,
                        "verdict": port.verdict,
                        "holders": port.holders.iter().map(|holder| json!({
                            "pid": holder.pid,
                            "comm": holder.comm,
                            "owner": holder.owner,
                            "owner_state": holder.owner_state,
                            "owner_declared": (holder.owner_state == OWNER_RESOLVED)
                                .then(|| declared_owners(&holder.owner)),
                        })).collect::<Vec<Value>>(),
                    })
                })
                .collect(),
        ),
    );
    object
}

/// Ask one already-resolved host whether this unit is the process on its ports.
pub async fn read_serving(
    target: &ComputeTarget,
    unit: &str,
    unit_path: &str,
    ports: &[u16],
    runner: &Runner,
) -> Result<ServingReport, DeployError> {
    let script = super::service::serving_script(unit, unit_path, &remote_serving_script(ports))?;
    let output = host_channel::run_script(target, &script, runner).await?;
    if !output.ok() {
        return Err(DeployError(format!(
            "{}: {}",
            target.name,
            host_channel::last_error_line(&output, "ssh failed")
        )));
    }
    parse_serving(&output.stdout)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn holder(pid: &str, owner: &str, state: &str) -> Holder {
        Holder {
            pid: pid.to_string(),
            comm: "/opt/homebrew/bin/node".to_string(),
            owner: owner.to_string(),
            owner_state: state.to_string(),
        }
    }

    fn report(launchd_pid: &str, ports: Vec<PortReport>) -> ServingReport {
        ServingReport {
            unit: "com.wisent.always-on.weles".to_string(),
            unit_path: "/Library/LaunchDaemons/com.wisent.always-on.weles.plist".to_string(),
            loaded: "yes".to_string(),
            launchd_pid: launchd_pid.to_string(),
            listeners_state: LISTENERS_READ.to_string(),
            ports,
        }
    }

    #[test]
    fn a_port_held_by_another_job_is_not_serving_and_names_that_job() {
        // The exact 2026-08-30 state: the declared unit is dead and the
        // undeclared unit the release deployer bootstraps holds 58101.
        let subject = report(
            "",
            vec![PortReport {
                port: 58101,
                holders: vec![holder("57910", "com.wisent.weles-worker", OWNER_RESOLVED)],
            }],
        );
        let ports = port_verdicts(&subject);
        assert_eq!(ports[0].verdict, PORT_SERVED_BY_OTHER);
        assert_eq!(verdict(&subject, &ports), SERVING_NO);
        let said = failure("charless-mac-mini", &subject, &ports).unwrap();
        assert!(said.contains("is not serving"), "{said}");
        assert!(said.contains("57910"), "{said}");
        assert!(said.contains("com.wisent.weles-worker"), "{said}");
    }

    #[test]
    fn identical_argv_under_another_label_never_counts_as_this_unit() {
        // Both units run the same program with the same arguments. Only the
        // label decides, so the pid must not be credited to the unit asked
        // about just because the pid launchd recorded is unknown here.
        let subject = report(
            "",
            vec![PortReport {
                port: 58101,
                holders: vec![holder("57910", "com.wisent.weles-worker", OWNER_RESOLVED)],
            }],
        );
        assert!(!belongs_to_unit(
            &subject.ports[0].holders[0],
            &subject.unit,
            &subject.launchd_pid
        ));
    }

    #[test]
    fn the_units_own_process_is_serving() {
        let subject = report(
            "4242",
            vec![PortReport {
                port: 58101,
                holders: vec![holder("4242", "com.wisent.always-on.weles", OWNER_RESOLVED)],
            }],
        );
        let ports = port_verdicts(&subject);
        assert_eq!(ports[0].verdict, PORT_SERVED_BY_UNIT);
        assert_eq!(verdict(&subject, &ports), SERVING_YES);
        assert_eq!(failure("h", &subject, &ports), None);
    }

    #[test]
    fn an_unresolvable_owner_is_unknown_and_never_someone_elses_port() {
        // A system LaunchDaemon is invisible to an unprivileged `launchctl
        // list`. Calling that "held by another job" would report every working
        // daemon as broken; calling it `serving` would repeat the original
        // defect. It is neither.
        let subject = report(
            "",
            vec![PortReport {
                port: 8788,
                holders: vec![holder("7438", "", OWNER_UNKNOWN)],
            }],
        );
        let ports = port_verdicts(&subject);
        assert_eq!(ports[0].verdict, PORT_OWNER_UNKNOWN);
        assert_eq!(verdict(&subject, &ports), SERVING_UNKNOWN);
        let said = failure("h", &subject, &ports).unwrap();
        assert!(said.contains("could not be established"), "{said}");
    }

    #[test]
    fn a_dead_port_is_not_serving_and_says_which_one() {
        let subject = report(
            "4242",
            vec![PortReport {
                port: 58101,
                holders: Vec::new(),
            }],
        );
        let ports = port_verdicts(&subject);
        assert_eq!(ports[0].verdict, PORT_DEAD);
        assert_eq!(verdict(&subject, &ports), SERVING_NO);
        assert!(failure("h", &subject, &ports)
            .unwrap()
            .contains("nothing is listening on 58101"));
    }

    #[test]
    fn an_unreadable_socket_table_is_unknown_not_dead() {
        let mut subject = report(
            "4242",
            vec![PortReport {
                port: 58101,
                holders: Vec::new(),
            }],
        );
        subject.listeners_state = LISTENERS_FAILED.to_string();
        let ports = port_verdicts(&subject);
        assert_eq!(ports[0].verdict, PORT_UNKNOWN);
        assert_eq!(verdict(&subject, &ports), SERVING_UNKNOWN);
        assert!(failure("h", &subject, &ports)
            .unwrap()
            .contains("socket table could not be read"));
    }

    #[test]
    fn the_script_carries_ports_only_base64_and_bounds_the_owner_walk() {
        let script = remote_serving_script(&[58101, 8788]);
        assert!(!script.contains("58101 8788"), "{script}");
        assert!(script.contains(&STANDARD.encode("58101 8788")));
        assert!(script.contains(&MAX_OWNER_DEPTH.to_string()));
        // The walk is what makes a launcher's child attributable to its job.
        assert!(script.contains("resolve_owner"), "{script}");
    }
}
