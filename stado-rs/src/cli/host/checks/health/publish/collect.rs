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
use crate::deploy::service_label_print::{inspect_label_carrying, LabelState};

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
        // The supervisor's own account of a unit it keeps restarting goes in
        // `detail`, which every reader of the beacon shows, so the crash loop
        // is named and not only the word `failed`. A job found while another
        // domain refused its read is real and incomplete: that is said too.
        let details: Vec<String> = state
            .failing()
            .into_iter()
            .chain(state.read_failure_detail())
            .collect();
        if !details.is_empty() {
            entry.insert("detail".to_string(), Value::String(details.join("; ")));
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

/// `stado host beacon collect [--publish]` — build this machine's health
/// beacon from the registry's declarations and the init system's answers.
///
/// Without `--publish` the document is printed and nothing is sent, so the
/// collection can be read on a host that holds no beacon grant.
pub async fn collect_beacon(publish: bool) -> Result<(), CmdError> {
    let document = collect_document().await?;
    super::publish_document(document, !publish).await
}

/// Build this machine's health beacon and deliver it to `destination`; the
/// `--health-interval-seconds` role of the host process, which hands the
/// store it serves and `promise`, how soon it publishes again.
pub async fn collect_beacon_to(
    destination: super::Destination<'_>,
    promise: std::time::Duration,
) -> Result<(), CmdError> {
    let mut document = collect_document().await?;
    super::deliver_document(&mut document, destination, Some(promise)).await
}

async fn collect_document() -> Result<Value, CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let target = crate::providers::local::agent::lookup_self_auto(&hostname)
        .await
        .map_err(CmdError::from)?
        .ok_or_else(|| {
            CmdError::click(format!(
                "{hostname} is not a registry target on this fleet, so there is nothing declared \
                 here to collect a beacon about"
            ))
            .stating(crate::primitives::failure::FailureCode::NotFound)
        })?;
    let runner = crate::deploy::production_runner();

    let mut units = Map::new();
    for service in crate::deploy::service::declared_services(&target) {
        let unit = service.unit_id();
        if unit.is_empty() || unit.contains(char::is_whitespace) || unit.contains(',') {
            continue;
        }
        // The carrying read: launchd's event log, the costly half, is read
        // only when the unit is not as it was at the previous pass and the
        // host is under its scan load ceiling.
        let entry = match inspect_label_carrying(
            &target,
            unit,
            crate::deploy::service::BootoutScope::Any,
            &runner,
        )
        .await
        {
            Ok(state) => {
                let mut entry = unit_entry(&state);
                if let Value::Object(fields) = &mut entry {
                    super::runner_listener::apply(
                        fields,
                        state.program.as_deref(),
                        state.pid.as_deref(),
                    );
                    // How the unit's launchd events were obtained this pass:
                    // `ok` for a read, `carried: …` for the previous read's.
                    if let Some(status) = &state.event_read_status {
                        fields.insert("events".to_string(), Value::String(status.clone()));
                    }
                }
                entry
            }
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
    // The memory line is the host's own kernel reading, the same fields its
    // capacity publication carries.
    if let Some(object) = document.as_object_mut() {
        object.insert(
            "memory".to_string(),
            Value::Object(crate::providers::local::host_memory::publication_fields(
                &crate::providers::local::host_memory::read_host_memory(),
            )),
        );
        // What macOS lets this process read, measured by this process.
        object.insert("privacy".to_string(), super::privacy::measure());
        // One process per role: the roles held on this host now, and the
        // Stado role processes nothing accounts for — a second copy of a
        // role, a build run from a checkout, a program since removed, a
        // retired command — ended in this pass and published as such, so a
        // stray never outlives one beacon and the register says what ran
        // beside the declared units rather than `active` from an old read.
        object.insert("roles".to_string(), roles_and_strays(&home)?);
    }
    Ok(document)
}

fn roles_and_strays(home: &std::path::Path) -> Result<Value, CmdError> {
    let held: Vec<Value> = crate::deploy::role_lock::holders(home)?
        .into_iter()
        .map(|holder| {
            json!({
                "role": holder.role,
                "pid": holder.pid,
                "program": holder.program,
                "since": holder.since,
            })
        })
        .collect();
    let strays = crate::deploy::role_lock::strays(home)?;
    let ended = crate::deploy::role_lock::end_strays(&strays)?;
    for stray in &ended {
        eprintln!(
            "[stado serve host-health] ended stray pid {} ({}), running {}: {}",
            stray.pid, stray.command, stray.running_for, stray.detail
        );
    }
    Ok(json!({
        "held": held,
        "ended_strays": ended,
    }))
}
