use crate::{
    catalog::text,
    common::{capture, checked, stado},
    state::ProductState,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

pub fn ensure(product: &Value, recipe: &Value, host: &str) -> Result<Value> {
    if product["service"]["installable"] != true {
        bail!(
            "{} has no installable managed-service declaration",
            text(product, "id")?
        );
    }
    if let Some(configuration) = recipe.get("host_config") {
        for (key, value) in configuration
            .as_object()
            .context("host_config must be an object")?
        {
            let value = value
                .as_str()
                .map(str::to_owned)
                .unwrap_or_else(|| value.to_string());
            checked(stado().args(["host", "config-set", host, key, &value]))?;
        }
    }
    let output = checked(stado().args([
        "service",
        "ensure",
        text(product, "id")?,
        "--host",
        host,
        "--reason",
        "installed from the canonical Wisent product catalog",
        "--json",
    ]))?;
    let receipt: Value = serde_json::from_slice(&output.stdout)
        .context("managed service ensure returned invalid JSON")?;
    let observed = observe(product, host)?;
    if observed["ready"] != true {
        bail!("service activation did not establish port ownership: {observed}");
    }
    let retired = retire_predecessors(product, host)?;
    Ok(json!({"activation": receipt, "observed": observed, "retired": retired}))
}

/// Withdraw every unit the catalog lists as this product's predecessor on
/// `host`, after its one service is ready: a declaration left in the registry
/// is one the reconciler starts again beside it.
///
/// A retired unit Stado does not manage is adopted first and then removed:
/// the catalog's word that it is retired is the authority, and reporting it
/// as "not managed" left `com.wisent.transcript-lake-secret-scrub`, installed
/// by hand on 2026-08-31, loaded on lukasz-macbook for four weeks after the
/// catalog retired it. A unit the host does not have is reported as absent.
/// Any other refusal fails the install, since the host would still run two
/// processes of the product.
fn retire_predecessors(product: &Value, host: &str) -> Result<Vec<Value>> {
    let mut outcomes = Vec::new();
    for unit in product["service"]["retired_units"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let unit = unit
            .as_str()
            .context("service.retired_units must hold unit names")?;
        let removed = remove_unit(unit, host)?;
        if removed.status.success() {
            outcomes.push(json!({"unit": unit, "status": "removed", "receipt": receipt(&removed)}));
            continue;
        }
        let stderr = String::from_utf8_lossy(&removed.stderr);
        if !stderr.contains("is not a registry-managed service on") {
            bail!(
                "could not retire {unit} on {host}; stado service remove exited {}: {}{}",
                removed.status,
                String::from_utf8_lossy(&removed.stdout),
                stderr.trim()
            );
        }
        let adopted = capture(stado().args(["service", "adopt", unit, "--host", host, "--json"]))?;
        let adoption = String::from_utf8_lossy(&adopted.stderr);
        if !adopted.status.success() {
            if adoption.contains("is not present on") {
                outcomes.push(json!({"unit": unit, "status": "absent", "detail": adoption.trim()}));
                continue;
            }
            bail!(
                "could not retire {unit} on {host}: it is not managed, and stado service adopt \
                 exited {}: {}",
                adopted.status,
                adoption.trim()
            );
        }
        let removed = remove_unit(unit, host)?;
        if !removed.status.success() {
            bail!(
                "adopted {unit} on {host} to retire it, and stado service remove then exited {}: {}",
                removed.status,
                String::from_utf8_lossy(&removed.stderr).trim()
            );
        }
        outcomes.push(json!({
            "unit": unit,
            "status": "adopted-and-removed",
            "adoption": receipt(&adopted),
            "receipt": receipt(&removed),
        }));
    }
    Ok(outcomes)
}

fn remove_unit(unit: &str, host: &str) -> Result<std::process::Output> {
    capture(stado().args(["service", "remove", unit, "--host", host, "--json"]))
}

fn receipt(output: &std::process::Output) -> Value {
    serde_json::from_slice::<Value>(&output.stdout).unwrap_or(Value::Null)
}

pub fn observe(product: &Value, host: &str) -> Result<Value> {
    let id = text(product, "id")?;
    let output = capture(stado().args(["service", "serving", id, "--host", host, "--json"]))?;
    let payload = serde_json::from_slice::<Value>(&output.stdout).unwrap_or(Value::Null);
    let ready = output.status.success()
        && payload.as_array().is_some_and(|rows| {
            !rows.is_empty() && rows.iter().all(|row| row["serving"] == "serving")
        });
    Ok(
        json!({"ready": ready, "operation": "stado service serving", "host": host, "exit_status": output.status.code(),
        "observed": payload, "stdout": String::from_utf8_lossy(&output.stdout), "stderr": String::from_utf8_lossy(&output.stderr),
        "source_attestation": "port ownership does not establish the remote executable's source revision"}),
    )
}

pub fn remove(product: &Value, state: &ProductState, host: &str) -> Result<()> {
    if state
        .host
        .as_deref()
        .is_some_and(|recorded| recorded != host)
    {
        bail!("service removal host differs from its recorded installation");
    }
    let unit = product["service"]["unit"]
        .as_str()
        .or_else(|| {
            state
                .extra
                .get("service")
                .and_then(|v| v["activation"]["unit"].as_str())
        })
        .context("service receipt and catalog do not identify the managed unit to remove")?;
    checked(stado().args(["service", "remove", unit, "--host", host, "--json"]))?;
    Ok(())
}
