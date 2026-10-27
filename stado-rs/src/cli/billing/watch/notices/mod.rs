//! A provider notice that asks for action is a defect of the dependency it
//! names, recorded in Oko in the same pass that read it. A failed payment or
//! a pending shutdown announced by mail days before the API refuses is only
//! a risk the fleet knows about once it stands in the defect register; a
//! line in a watch report nobody reads is not.
//!
//! Each notice is recorded once: the snapshot carries, under
//! [`RECORDED_KEY`], every Skrzynka message id already recorded with Oko's
//! answer, and a later pass that reads the same message again skips it. A
//! notice Oko refused to record is kept with the refusal and tried again on
//! the next pass, so a register that was unreachable loses nothing.

use serde_json::{json, Map, Value};

use super::mail::{sender_domain, MailProbe};
use crate::mail::MailAnalysis;

/// The snapshot member holding the recorded notices: message id to Oko's
/// answer (`{"defect": …}` or `{"refused": …}`).
pub(super) const RECORDED_KEY: &str = "provider_notices_recorded";

/// One notice this pass handled: the defect Oko recorded for it, or why it
/// did not.
pub(super) struct Recorded {
    pub(super) message_id: String,
    pub(super) subject: String,
    pub(super) outcome: Result<String, String>,
}

impl Recorded {
    pub(super) fn as_value(&self) -> Value {
        match &self.outcome {
            Ok(defect) => {
                json!({ "message_id": self.message_id, "subject": self.subject, "defect": defect })
            }
            Err(refused) => {
                json!({ "message_id": self.message_id, "subject": self.subject, "refused": refused })
            }
        }
    }
}

/// Record every action-required notice in `mail` that `previous` does not
/// already hold as recorded, and carry the whole recorded map into
/// `document` so the next pass knows them.
pub(super) fn record(
    previous: Option<&Value>,
    document: &mut Value,
    mail: &MailProbe,
) -> Vec<Recorded> {
    let mut recorded: Map<String, Value> = match previous.map(|snapshot| &snapshot[RECORDED_KEY]) {
        Some(Value::Object(map)) => map.clone(),
        Some(_) | None => Map::new(),
    };
    let outcomes: Vec<Recorded> = mail
        .action_required()
        .into_iter()
        .filter_map(|notice| {
            if already_recorded(&recorded, &notice.id) {
                return None;
            }
            let outcome = record_one(notice);
            let answer = match &outcome {
                Ok(defect) => json!({ "defect": defect, "subject": notice.subject }),
                Err(refused) => json!({ "refused": refused, "subject": notice.subject }),
            };
            recorded.insert(notice.id.clone(), answer);
            Some(Recorded {
                message_id: notice.id.clone(),
                subject: notice.subject.clone(),
                outcome,
            })
        })
        .collect();
    if let Value::Object(members) = document {
        members.insert(RECORDED_KEY.to_string(), Value::Object(recorded));
    }
    outcomes
}

/// Whether `message_id` already stands in the map with a recorded defect; a
/// refused record is not recorded, so it is tried again.
fn already_recorded(recorded: &Map<String, Value>, message_id: &str) -> bool {
    matches!(recorded.get(message_id), Some(Value::Object(answer)) if answer.contains_key("defect"))
}

/// `oko defects record` for one notice, run from the path a Stado install
/// places Oko at, since the unit that runs the watch carries no login
/// `PATH`. `Ok` with the defect id Oko printed, `Err` with why it did not.
fn record_one(notice: &MailAnalysis) -> Result<String, String> {
    let oko = crate::deploy::host_exec::installed_program(crate::deploy::host_exec::OKO_CLI)
        .map_err(|missing| format!("oko defects record could not run: {missing}"))?;
    let provider = match sender_domain(&notice.from) {
        Some(domain) => domain,
        None => {
            return Err(format!(
                "the notice's sender {:?} carries no domain, so the dependency it concerns cannot be named",
                notice.from
            ))
        }
    };
    let host = crate::providers::vast::system_hostname();
    let actions = if notice.action_signals.is_empty() {
        match &notice.reading_error {
            Some(error) => {
                format!("the notice could not be read ({error}); the operator reads it himself")
            }
            None => "the notice asks for action without naming it".to_string(),
        }
    } else {
        notice.action_signals.join("; ")
    };
    let surface = format!("{provider}: a service the fleet declares on this provider");
    let title = format!("{provider} notice: {}", notice.subject);
    let found_in = format!("stado billing watch on {host}");
    let impact = format!(
        "{provider} announced an action the account owes; when it is not taken the provider suspends what the fleet declares on it, and every consumer resolved there fails at once"
    );
    let evidence = format!(
        "received {} from {}: {}; read with `skrzynka message show {}`",
        notice.received_at, notice.from, actions, notice.id
    );
    let repair = format!(
        "move what the fleet declares on {provider} onto the fleet (a database: `stado database migrate <name> --to fleet`) or settle the account as the notice asks; then the next `stado billing watch` pass reads no newer notice from {provider}"
    );
    let mut command = std::process::Command::new(&oko);
    command.args([
        "defects",
        "record",
        "--product",
        "stado",
        "--surface",
        &surface,
        "--title",
        &title,
        "--found-in",
        &found_in,
        "--impact",
        &impact,
        "--evidence",
        &evidence,
        "--repair",
        &repair,
    ]);
    match crate::wait::output(&mut command) {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
        }
        Ok(output) => Err(format!(
            "oko defects record ended {}: {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        )),
        Err(error) => Err(format!(
            "oko defects record could not run {}: {error}",
            oko.display()
        )),
    }
}
