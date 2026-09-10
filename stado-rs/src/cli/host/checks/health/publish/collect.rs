//! Collecting THIS host's health beacon: what the registry declares here,
//! what the init system holds under each of those identities, and how much
//! room the volume the fleet writes to has left.
//!
//! Why the product owns this and a host script no longer may: the collector
//! that used to build this document read each label with
//! `launchctl print … 2>/dev/null || true` and published `inactive` when the
//! read produced nothing. A read the host refuses produces nothing, so a
//! daemon that was loaded and running was published as not loaded — and
//! `stado service status`, `registry doctor` and Stado Desktop all repeated
//! it, because a beacon is the only thing they have. The refusal and the
//! absence are two different facts here, and they stay two facts all the way
//! to the operator's screen.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::deploy::service::{
    STATE_ACTIVE, STATE_FAILED, STATE_INACTIVE, STATE_UNKNOWN, STATE_UNREADABLE,
};
use crate::deploy::service_label_print::{inspect_label, LabelState};

/// Read one declared identity's state the way an operator would read it, and
/// keep the reason when the answer is not a state.
fn unit_entry(state: &LabelState) -> Value {
    if let Some(system) = &state.unsupported {
        return json!({"state": STATE_UNKNOWN, "detail": format!("this host runs {system}")});
    }
    if state.loaded() {
        let running = state.state.as_deref() == Some("running");
        let failed = !running
            && state
                .last_exit_code
                .as_deref()
                .is_some_and(|code| !matches!(code.trim(), "" | "0" | "(never exited)"));
        let mut entry = Map::new();
        entry.insert(
            "state".to_string(),
            Value::String(if failed { STATE_FAILED } else { STATE_ACTIVE }.to_string()),
        );
        if let Some(domain) = &state.domain {
            entry.insert("domain".to_string(), Value::String(domain.clone()));
        }
        if let Some(pid) = &state.pid {
            entry.insert("pid".to_string(), Value::String(pid.clone()));
        }
        // A job was found and another domain still refused its read: the
        // state is real, and the answer is not complete.
        if let Some(detail) = state.read_failure_detail() {
            entry.insert("detail".to_string(), Value::String(detail));
        }
        return Value::Object(entry);
    }
    match state.read_failure_detail() {
        // Nothing answered, and at least one domain would not be read. The
        // one thing this must never say is `inactive`.
        Some(detail) => json!({"state": STATE_UNREADABLE, "detail": detail}),
        None => json!({"state": STATE_INACTIVE}),
    }
}

/// `stado host collect-beacon [--publish]` — build this machine's health
/// beacon from the registry's declarations and the init system's answers.
///
/// Without `--publish` the document is printed and nothing is sent, so the
/// collection can be read on a host that holds no beacon grant.
pub async fn collect_beacon(publish: bool) -> Result<(), CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let target = crate::providers::local::agent::lookup_self_auto(&hostname)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?
        .ok_or_else(|| {
            CmdError::click(format!(
                "{hostname} is not a registry target on this fleet, so there is nothing declared \
                 here to collect a beacon about"
            ))
        })?;
    let runner = crate::deploy::production_runner();

    let mut units = Map::new();
    for service in crate::deploy::service::declared_services(&target) {
        let unit = service.unit_id();
        if unit.is_empty() || unit.contains(char::is_whitespace) || unit.contains(',') {
            continue;
        }
        let entry = match inspect_label(
            &target,
            unit,
            crate::deploy::service::BootoutScope::Any,
            &runner,
        )
        .await
        {
            Ok(state) => unit_entry(&state),
            // The read itself could not be run. That is this host's own
            // failure to describe, never another word for `inactive`.
            Err(error) => json!({"state": STATE_UNREADABLE, "detail": error.to_string()}),
        };
        units.insert(unit.to_string(), entry);
    }

    let mut document = json!({
        "host": super::beacon_slug(&hostname),
        "reported_at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true),
        "units": Value::Object(units),
    });

    // The same volume the janitor measures and the same call it measures it
    // with, so a beacon and a reclaim pass can never disagree about free
    // space. `stado host health` prints these two numbers; the old shell
    // collector published a `df -h` line instead and both read as `-`.
    let home = crate::config_file::expand_tilde("~");
    if let Ok(free) = crate::providers::local::disk_cleanup::free_bytes(&home) {
        if let Some(object) = document.as_object_mut() {
            let free_gb = free / i64::from(1024).pow(3);
            object.insert("disk_avail_gb".to_string(), json!(free_gb));
            object.insert(
                "disk".to_string(),
                Value::String(format!("{free_gb} GiB available on {}", home.display())),
            );
        }
    }
    // The memory line is a read of the pass, not a fourth measurement: the
    // janitor and the queue agent both write the reading they decided
    // against, and a beacon measuring its own would publish a number no
    // watermark was applied to.
    if let Some(object) = document.as_object_mut() {
        object.insert(
            "memory".to_string(),
            crate::providers::local::host_memory::report::last_report_in(&home),
        );
    }
    super::publish_document(document, !publish).await
}
