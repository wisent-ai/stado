//! What the host does with a service that stands: the declared repair step
//! of its repair declaration, when the answering declaration names one,
//! then the declared restart through the same path `stado service restart`
//! takes, then the record and the alert. No manual step: the host that runs
//! the service is the one that acts, on the beacon pass that found it
//! standing, and the next pass asks the new process afresh.
//!
//! A recovery that fails at the repair still restarts: the repair is what
//! clears the known causes, the restart is what ends the process that holds
//! the requests. A recovery whose restart fails is recorded as such and
//! alerted, and the beacon carries the refusal, so a service that cannot be
//! restarted does not read as recovered.

use serde_json::{json, Value};

use super::register::{self, Recovery};
use super::Reading;
use crate::cli::CmdError;
use crate::deploy::service_catalog::CatalogService;

/// The alert's subject.
const SUBJECT: &str = "Stado recovered a service that stopped answering";

/// Recover `entry`, which `reading` found standing on `host`, and record
/// what was done. Answers the record as JSON for the beacon.
pub async fn recover(
    home: &std::path::Path,
    host: &str,
    entry: &CatalogService,
    unit: &str,
    reading: &Reading,
) -> Result<Value, CmdError> {
    let Reading::Stood {
        since,
        waited_seconds,
        bound_seconds,
    } = reading
    else {
        return Err(CmdError::refused(format!(
            "{} does not stand ({}), so there is nothing to recover",
            entry.name,
            reading.document()
        )));
    };
    let declared = entry.answering.as_ref().ok_or_else(|| {
        CmdError::refused(format!(
            "{} declares no answering question, so nothing judged it standing",
            entry.name
        ))
    })?;
    let held_requests = match crate::cli::resolver::held_to_service(&entry.name) {
        Ok(held) => held,
        Err(cause) => vec![json!({ "unreadable": cause })],
    };
    let repair = match &declared.repair {
        None => Value::Null,
        Some(step) => match crate::cli::repair::apply_step(&entry.name, step, host).await {
            Ok(report) => report,
            Err(error) => {
                json!({ "name": step, "status": "refused", "refusal": error.to_string() })
            }
        },
    };
    let (restart, restarted) = match crate::cli::service::lifecycle::release::unit::restart_quietly(
        &entry.name,
        Some(host),
    )
    .await
    {
        Ok(report) => (report, true),
        Err(error) => (json!({ "refusal": error.to_string() }), false),
    };
    super::forget(&entry.name);
    let message = format!(
        "{host}: {} ({unit}) answered nothing on {} {} for {waited_seconds}s, past the {bound_seconds}s \
         bound; {} request(s) stood on this host's resolver; repair {}; restart {}. `stado service \
         recoveries` holds the record.",
        entry.name,
        declared.method,
        declared.path,
        held_requests.len(),
        match &repair {
            Value::Null => String::from("not declared"),
            report => report["status"].to_string(),
        },
        if restarted { "ended well" } else { "refused" }
    );
    let alert: Vec<Value> = crate::monitor::alerts::send_alert(crate::config::alerts_topic(), &message, SUBJECT)
        .await
        .iter()
        .map(|delivery| {
            json!({ "channel": delivery.channel, "delivered": delivery.delivered, "detail": delivery.detail })
        })
        .collect();
    let recovery = Recovery {
        at: super::now_iso(),
        host: host.to_string(),
        service: entry.name.clone(),
        unit: unit.to_string(),
        question: format!("{} {}", declared.method, declared.path),
        stood_since: since.clone(),
        stood_seconds: *waited_seconds,
        bound_seconds: *bound_seconds,
        held_requests,
        repair,
        restart,
        restarted,
        alert: Value::Array(alert),
    };
    register::append(home, &recovery)?;
    Ok(json!(recovery))
}
