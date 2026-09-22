//! A fleet exception is read from Tama's verified build registry, never from
//! an agent-supplied boolean or an unverified quote in a queue request.

use serde_json::{json, Value};

pub const REGISTRY_KEY: &str = "build_approvals";

pub struct BuildIntent<'a> {
    pub product: &'a str,
    pub revision: &'a str,
    pub platform: &'a str,
}

pub async fn verify(intent: &BuildIntent<'_>) -> Result<Value, String> {
    let output = tokio::process::Command::new("tama-cli")
        .args([
            "build",
            "approval",
            "--target",
            intent.product,
            "--revision",
            intent.revision,
            "--json",
        ])
        .kill_on_drop(true)
        .output()
        .await
        .map_err(|error| {
            format!("cannot verify recorded user build approval through Tama: {error}")
        })?;
    if !output.status.success() {
        return Err(format!(
            "Tama refused the build exception ({}): {}",
            output.status,
            String::from_utf8_lossy(&output.stderr).trim()
        ));
    }
    let entry: Value = serde_json::from_slice(&output.stdout)
        .map_err(|error| format!("Tama returned unreadable build approval: {error}"))?;
    if entry["target"].as_str() != Some(intent.product)
        || entry["revision"].as_str() != Some(intent.revision)
        || !entry["user_approval"].is_object()
    {
        return Err("Tama returned approval for different build coordinates".into());
    }
    Ok(entry)
}

fn receipt_key(entry: &Value, platform: &str) -> Result<String, String> {
    let approval = &entry["user_approval"];
    let field = |name: &str| {
        approval[name]
            .as_str()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| format!("verified build approval is missing {name}"))
    };
    Ok(serde_json::to_string(&(
        field("session_id")?,
        field("turn_digest")?,
        field("target")?,
        platform,
    ))
    .expect("string tuple serializes"))
}

/// A worker may claim tomorrow a job that was authorized today. Its exact
/// submission remains paid for even after the daily counter rolls over.
pub fn charged(document: &Value, run_id: &str) -> bool {
    document
        .get(REGISTRY_KEY)
        .and_then(Value::as_object)
        .is_some_and(|receipts| {
            receipts
                .values()
                .any(|receipt| receipt["run_id"].as_str() == Some(run_id))
        })
}

pub fn record(
    document: &mut Value,
    entry: Value,
    platform: &str,
    run_id: &str,
) -> Result<(), String> {
    let key = receipt_key(&entry, platform)?;
    let root = document
        .as_object_mut()
        .ok_or("registry is not an object")?;
    let receipts = root
        .entry(REGISTRY_KEY)
        .or_insert_with(|| json!({}))
        .as_object_mut()
        .ok_or("build approval receipts are not an object")?;
    if let Some(previous) = receipts.get(&key) {
        if previous["run_id"].as_str() != Some(run_id) {
            return Err(format!("this user message already authorized another {platform} build of {}; a retry needs new consent", entry["target"]));
        }
        return Ok(());
    }
    receipts.insert(
        key,
        json!({"run_id": run_id, "platform": platform,
        "recorded_at": chrono::Utc::now().to_rfc3339(), "entry": entry}),
    );
    Ok(())
}
