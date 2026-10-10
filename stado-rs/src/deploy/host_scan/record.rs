//! What this process's scans last decided, for the beacon: every scan that
//! is standing skipped, with the reading that refused it and since when,
//! and the last reading any scan was decided on.

use std::collections::BTreeMap;
use std::sync::{LazyLock, Mutex, MutexGuard};

use chrono::{SecondsFormat, Utc};
use serde::Serialize;
use serde_json::Value;

use super::admission::Admission;

/// One scan standing skipped.
#[derive(Clone, Debug, Serialize)]
pub struct Skip {
    /// When the scan was first refused in the run of refusals that still
    /// stands; an admission in between ends the run.
    pub since: String,
    /// The latest refusal's reading.
    pub load: f64,
    pub ceiling: f64,
    pub ceiling_source: super::admission::CeilingSource,
    /// When the latest refusal was.
    pub latest: String,
}

#[derive(Default, Serialize)]
struct Register {
    skipped: BTreeMap<String, Skip>,
    last: Option<Admission>,
}

static REGISTER: LazyLock<Mutex<Register>> = LazyLock::new(Mutex::default);

/// The register, or the sentence that says a scan's thread died holding it.
fn register() -> Result<MutexGuard<'static, Register>, String> {
    REGISTER.lock().map_err(|poisoned| {
        format!(
            "the scan register is poisoned: a scan ended while recording its verdict ({poisoned})"
        )
    })
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Secs, true)
}

pub(super) fn skipped(scan: &str, admission: &Admission) -> Result<(), String> {
    let mut register = register()?;
    let at = now();
    let entry = register
        .skipped
        .entry(scan.to_string())
        .or_insert_with(|| Skip {
            since: at.clone(),
            load: admission.load,
            ceiling: admission.ceiling,
            ceiling_source: admission.ceiling_source,
            latest: at.clone(),
        });
    entry.load = admission.load;
    entry.ceiling = admission.ceiling;
    entry.ceiling_source = admission.ceiling_source;
    entry.latest = at;
    register.last = Some(admission.clone());
    Ok(())
}

pub(super) fn admitted(scan: &str, admission: &Admission) -> Result<(), String> {
    let mut register = register()?;
    register.skipped.remove(scan);
    register.last = Some(admission.clone());
    Ok(())
}

/// The `scans` block of this host's beacon: the last reading and every scan
/// standing skipped (`last` is `null` while no scan has asked yet), or the
/// sentence that says the register could not be read.
pub fn published() -> Result<Value, String> {
    let register = register()?;
    serde_json::to_value(&*register)
        .map_err(|error| format!("the scan register cannot be serialized: {error}"))
}

/// The block as a line for an operator: `load 3.2 under ceiling 12
/// (measured_cores)`, with every standing skip named, or `-` before any scan
/// has asked.
pub fn line(block: &Value) -> String {
    let Some(last) = block.get("last").filter(|last| !last.is_null()) else {
        return "-".to_string();
    };
    let number = |key: &str| {
        last.get(key)
            .and_then(Value::as_f64)
            .map_or_else(|| "?".to_string(), |value| value.to_string())
    };
    let source = last
        .get("ceiling_source")
        .and_then(Value::as_str)
        .map_or_else(|| "?".to_string(), str::to_string);
    let mut sentence = format!(
        "load {} against ceiling {} ({source})",
        number("load"),
        number("ceiling")
    );
    if let Some(skipped) = block.get("skipped").and_then(Value::as_object) {
        for (scan, skip) in skipped {
            let since = skip
                .get("since")
                .and_then(Value::as_str)
                .map_or_else(|| "?".to_string(), str::to_string);
            sentence.push_str(&format!("; {scan} skipped since {since}"));
        }
    }
    sentence
}
