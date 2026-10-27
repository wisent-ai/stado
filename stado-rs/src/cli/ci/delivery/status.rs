//! The delivery routine as `stado ci status` and Stado Desktop show it.

use chrono::Utc;
use serde_json::{json, Value};

use super::{plan, state};
use crate::cli::CmdError;
use crate::cli::reporting::field;

/// The plan, the step running and how long it has run, the refused step
/// with every refusal and its defects, what each step last delivered, and
/// what the main check found on each step's `main`.
pub fn status() -> Result<Value, CmdError> {
    let current = state::read()?;
    let running = current.running.as_ref().map(|running| {
        json!({
            "step": running.step,
            "revision": running.revision,
            "started_at": running.started_at,
            "age_seconds": (Utc::now() - running.started_at).num_seconds(),
        })
    });
    Ok(json!({
        "plan": plan::read()?,
        "plan_path": plan::path()?,
        "state_path": state::path()?,
        "running": running,
        "refused": current.refused,
        "delivered": current.delivered,
        "pushed": current.pushed,
        "last_pass": current.last_pass,
        "main": crate::cli::ci::main::document()?,
    }))
}

/// `stado ci delivery`: [`path`] as JSON or lines. Exits zero whether or not
/// the path stands; a state it cannot read is the refusal.
pub async fn delivery(json: bool) -> Result<(), CmdError> {
    let document = path().await?;
    if json {
        println!("{}", serde_json::to_string_pretty(&document)?);
    } else {
        print_path(&document);
    }
    Ok(())
}

/// Whether the path that delivers repairs stands on this machine, for
/// `stado ci delivery` and Oko's order of work: the routine's state, who
/// holds its pass lock, which product operations hold build trees, and this
/// machine's Stado unit as its init system holds it — the unit whose
/// coordinator tick fires a pass on a host that declares a plan, so a failing
/// unit is a routine nothing fires. `stalled` names every way the path is
/// down now, each in one sentence; empty when it stands.
/// `unrecorded` names every write in Oko's register about what the routine
/// now stands on (each step's last checked commit, the refused step, each
/// step's last install) that the register refused: the routine went on
/// without it, so the register lacks what happened until it is recorded.
pub async fn path() -> Result<Value, CmdError> {
    let routine = status()?;
    let current = state::read()?;
    let holder = state::lock_holder()?;
    let label = crate::cli::web::unit_label(env!("CARGO_PKG_NAME"));
    let unit = unit(&label).await;
    let mut stalled = Vec::new();
    if let Some(refused) = &current.refused {
        stalled.push(format!(
            "step {} at {} was refused on {}: {}",
            refused.step,
            refused.revision,
            refused.at.to_rfc3339(),
            refused.refusals.join("; ")
        ));
    }
    stalled.extend(crate::cli::ci::main::red(&routine["main"]));
    let mut unrecorded = crate::cli::ci::main::unrecorded(&routine["main"]);
    if let Some(refused) = &current.refused {
        for line in &refused.unrecorded {
            let (step, revision) = (&refused.step, &refused.revision);
            unrecorded.push(format!("step {step} refused at {revision}: {line}"));
        }
    }
    for (step, delivered) in &current.delivered {
        for line in &delivered.unrecorded {
            let revision = &delivered.revision;
            unrecorded.push(format!("step {step} installed at {revision}: {line}"));
        }
    }
    if let (Some(running), None) = (&current.running, &holder) {
        stalled.push(format!(
            "step {} at {} is recorded as installing since {}, but no pass holds the routine's lock: \
             the pass that ran it ended without recording an outcome",
            running.step,
            running.revision,
            running.started_at.to_rfc3339()
        ));
    }
    if let Some(failing) = unit["failing"].as_str() {
        stalled.push(format!("{label} on this machine is failing: {failing}"));
    }
    // The coordinator's tick fires a pass only on a host that declares a
    // plan, so a host without one, or with one it cannot read, runs no pass.
    match plan::read() {
        Ok(Some(_)) => {}
        Ok(None) => stalled.push(
            "this host declares no delivery plan (`stado ci plan set`), so the coordinator's tick fires no pass here"
                .to_owned(),
        ),
        Err(error) => stalled.push(format!(
            "this host's delivery plan cannot be read, so the coordinator's tick fires no pass here: {error}"
        )),
    }
    // A pass that could not finish checked and installed nothing past where
    // it stopped, and the path stays down until another pass is fired: said
    // as a stall, never left as the last pass's words alone.
    if let Some(pass) = &current.last_pass {
        if let Some(failed) = &pass.failed {
            stalled.push(format!(
                "the last pass, ended {}, could not finish, and no pass has finished since: {failed}",
                pass.ended_at.to_rfc3339()
            ));
        }
    }
    Ok(json!({
        "routine": routine,
        "pass_lock_holder": holder,
        "build_holds": crate::providers::local::space_reclaim::workload_holders(),
        "unit": unit,
        "blocking_defects": current.refused.as_ref().map(|refused| refused.defects.clone()),
        "stalled": stalled,
        "unrecorded": unrecorded,
    }))
}

/// `label` as this machine's init system holds it: loaded, state, pid, runs,
/// last exit code and why it is failing; a read that could not decide is
/// kept as `read_error`, never taken for a healthy unit. Only the state is
/// read, not launchd's event log: the path decides by the state, and Oko's
/// gates wait on this answer.
async fn unit(label: &str) -> Value {
    let target = match crate::deploy::service::local_target() {
        Ok(target) => target,
        Err(error) => return json!({ "label": label, "read_error": error.to_string() }),
    };
    let runner = crate::deploy::production_runner();
    let scope = match crate::deploy::service::BootoutScope::parse(None) {
        Ok(scope) => scope,
        Err(error) => return json!({ "label": label, "read_error": error.to_string() }),
    };
    match crate::deploy::service_label_print::inspect_label_state(&target, label, scope, &runner)
        .await
    {
        Ok(state) => json!({
            "label": label,
            "loaded": state.loaded(),
            "state": state.state,
            "pid": state.pid,
            "runs": state.runs,
            "last_exit_code": state.last_exit_code,
            "failing": state.failing(),
            "events": state.event_read_status,
        }),
        Err(error) => json!({ "label": label, "read_error": error.to_string() }),
    }
}

/// The path's lines in `stado ci delivery`'s plain form.
pub fn print_path(document: &Value) {
    print_status(&document["routine"]);
    match document["pass_lock_holder"].as_str() {
        Some(holder) => println!("  pass lock held by {holder}"),
        None => println!("  pass lock free"),
    }
    println!("  build trees held by: {}", field(&document["build_holds"]));
    let unit = &document["unit"];
    match unit["read_error"].as_str() {
        Some(error) => println!("  unit {}: could not be read: {error}", field(&unit["label"])),
        None => println!(
            "  unit {}: loaded {} state {} pid {} runs {} last exit code {}; events {}",
            field(&unit["label"]),
            field(&unit["loaded"]),
            field(&unit["state"]),
            field(&unit["pid"]),
            field(&unit["runs"]),
            field(&unit["last_exit_code"]),
            field(&unit["events"])
        ),
    }
    let stalled = document["stalled"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str);
    let mut down = false;
    for reason in stalled {
        down = true;
        println!("  stalled: {reason}");
    }
    if !down {
        println!("  the delivery path stands");
    }
    let unrecorded = document["unrecorded"].as_array().into_iter().flatten();
    for line in unrecorded.filter_map(Value::as_str) {
        println!("  register write refused: {line}");
    }
}

/// The routine's lines in `stado ci status`'s plain form.
pub fn print_status(document: &Value) {
    use crate::cli::reporting::field;
    match document["plan"]["steps"].as_array() {
        None => println!(
            "delivery routine: no plan declared at {}",
            field(&document["plan_path"])
        ),
        Some(steps) => {
            let names: Vec<String> = steps
                .iter()
                .map(|step| format!("{}:{}", field(&step["product"]), field(&step["surface"])))
                .collect();
            println!("delivery routine plan: {}", names.join(" -> "));
        }
    }
    // Each record is read by key from the document, so a key the record
    // lacks is said as not reported instead of ending the print.
    let running = &document["running"];
    if running.is_object() {
        println!(
            "  running: {} at {} for {}s",
            field(&running["step"]), field(&running["revision"]), field(&running["age_seconds"])
        );
    }
    let refused = &document["refused"];
    if refused.is_object() {
        println!(
            "  refused: {} at {} on {}, defects {}",
            field(&refused["step"]), field(&refused["revision"]), field(&refused["at"]), field(&refused["defects"])
        );
    }
    for (step, delivered) in document["delivered"].as_object().into_iter().flatten() {
        println!(
            "  delivered: {step} at {} on {}",
            field(&delivered["revision"]), field(&delivered["at"])
        );
        let unrepaired = delivered["unrepaired"].as_array().into_iter().flatten();
        for defect in unrepaired.filter_map(Value::as_str) {
            println!("    its repair of {defect} is still to be recorded in Oko");
        }
    }
    for (checkout, pushed) in document["pushed"].as_object().into_iter().flatten() {
        let (revision, at) = (field(&pushed["revision"]), field(&pushed["at"]));
        match pushed["refusal"].as_str() {
            Some(refusal) => println!("  push refused: {checkout} at {revision} on {at}: {refusal}"),
            None => println!("  pushed: {checkout} at {revision} on {at}"),
        }
    }
    let last = &document["last_pass"];
    if last.is_object() {
        println!("  last pass {}: {}", field(&last["ended_at"]), field(&last["said"]));
    }
    crate::cli::ci::main::print(&document["main"]);
}
