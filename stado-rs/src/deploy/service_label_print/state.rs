//! What an init system holds under one identity, and the questions a caller
//! asks of it. No host is touched here; this is the answer's shape.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Why a domain read produced no answer: the host refused it, or it failed.
///
/// The distinction is the whole point of the type. A refused read and an
/// absent unit look identical in a collector that drops stderr, and the
/// beacon that did exactly that published `inactive` for a loaded daemon.
pub const READ_REFUSED: &str = "permission_refused";
/// A read that reached the init system and failed for any other reason.
pub const READ_FAILED: &str = "read_failed";

fn read_failed() -> String {
    READ_FAILED.to_string()
}

/// A domain whose init-system state could not be read. This is distinct from
/// an authoritative answer that the named job is absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelReadFailure {
    pub domain: String,
    pub exit_code: i32,
    pub detail: String,
    /// [`READ_REFUSED`] when the host refused the read for want of privilege,
    /// [`READ_FAILED`] otherwise. Older reports carried no kind and are read
    /// as plain failures.
    #[serde(default = "read_failed")]
    pub kind: String,
}

impl LabelReadFailure {
    /// Was this read refused rather than answered?
    pub fn refused(&self) -> bool {
        self.kind == READ_REFUSED
    }
}

/// What an init system holds under one exact service identity.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LabelState {
    pub host: String,
    pub label: String,
    /// Domain reads that were refused or otherwise failed. An empty loaded
    /// result is authoritative only when this is also empty.
    pub read_failures: Vec<LabelReadFailure>,
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

    /// Whether the requested init-system domains were read conclusively.
    ///
    /// `not_loaded` is only ever said when every requested domain answered:
    /// a domain that refused the read leaves [`READ_REFUSED`] here instead,
    /// because "you may not look" is not "there is nothing there".
    pub fn read_status(&self) -> &'static str {
        if self.unsupported.is_some() {
            "unsupported"
        } else if self.loaded() {
            "loaded"
        } else if self.read_failures.is_empty() {
            "not_loaded"
        } else if self.read_failures.iter().all(LabelReadFailure::refused) {
            READ_REFUSED
        } else {
            "unavailable"
        }
    }

    /// Whether any domain refused its read, whatever else was found. A found
    /// job beside a refused domain is still an incomplete answer.
    pub fn refused_read(&self) -> bool {
        self.read_failures.iter().any(LabelReadFailure::refused)
    }

    /// Render the bounded domain failures for a CLI or higher-level refusal.
    pub fn read_failure_detail(&self) -> Option<String> {
        if self.read_failures.is_empty() {
            return None;
        }
        Some(
            self.read_failures
                .iter()
                .map(|failure| {
                    let outcome = if failure.refused() {
                        "refused the read, exit"
                    } else {
                        "exited"
                    };
                    format!(
                        "{} {outcome} {}: {}",
                        failure.domain, failure.exit_code, failure.detail
                    )
                })
                .collect::<Vec<_>>()
                .join("; "),
        )
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
            "read_status": self.read_status(),
            "read_failures": self.read_failures,
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
