//! Machine-initiated enrollment: `join`, `pending`, `approve`, `reject`.
//!
//! The machine being added knows everything about itself, so the flow
//! starts there: `stado_fleet join` announces the machine's real hostname,
//! OS and architecture as an `enrollments/<hostname>.json` request in the
//! configured store (and prints it, for setups where the store is not
//! shared and the request travels by any channel). The operator lists
//! requests with `pending` and turns one into a registered target with
//! `approve` — the same validated compare-and-swap registry write as every
//! other fleet command, so a colliding host identity is refused by the
//! registry-v2 contract, never papered over.
//!
//! Both `join` and `approve` honor the fleet's central enrollment catalog
//! (`registry.enrollment`, see [`catalog`]): a path the catalog disables
//! is refused.

pub mod catalog;
pub mod legacy;

use serde_json::{json, Value};
use stado::cli::registry::{fetch_document, push_document};
use stado::queue::JobStorage;
use stado::targets::normalize_hostname;

use crate::ops::register_target;

/// Store prefix every enrollment request lives under.
const REQUESTS_PREFIX: &str = "enrollments/";
/// Request lifecycle markers.
const STATUS_PENDING: &str = "pending";
const STATUS_APPROVED: &str = "approved";

/// Map one directly observed operating-system and architecture pair to the
/// closed immutable-release platform table. Both Rust's local constants and
/// the exact `uname` spellings used by remote enrollment are accepted.
pub fn release_platform(os: &str, arch: &str) -> Result<&'static str, String> {
    match (os.trim(), arch.trim()) {
        ("macos", "aarch64") | ("Darwin", "arm64") => Ok("darwin-arm64"),
        ("linux", "x86_64") | ("Linux", "x86_64") => Ok("linux-amd64"),
        (os, arch) => Err(format!(
            "unsupported release platform observation: os={os:?}, arch={arch:?}"
        )),
    }
}

fn request_path(hostname: &str) -> String {
    format!("{REQUESTS_PREFIX}{hostname}.json")
}

/// Build the join-request document for this machine. Pure.
pub fn build_request(hostname: &str, os: &str, arch: &str) -> Value {
    json!({
        "hostname": hostname,
        "os": os,
        "arch": arch,
        "kind": "local",
        "requested_at": chrono::Utc::now().to_rfc3339(),
        "status": STATUS_PENDING,
    })
}

/// Derive the target name for an approved request: the normalized machine
/// hostname. Pure.
pub fn target_name_for(hostname: &str) -> String {
    normalize_hostname(hostname)
}

/// Parse a stored request and confirm it is still awaiting a decision.
/// Pure.
pub fn pending_request(document: &Value) -> Result<&str, String> {
    let status = document
        .get("status")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if status != STATUS_PENDING {
        return Err(format!(
            "request status is '{status}', not '{STATUS_PENDING}'"
        ));
    }
    document
        .get("hostname")
        .and_then(Value::as_str)
        .filter(|hostname| !hostname.is_empty())
        .ok_or_else(|| "request has no hostname".to_string())
}

/// `stado_fleet join` — run on the machine being added. Announces itself
/// in the store and prints the request for carry-over setups.
pub async fn join() -> Result<bool, String> {
    let hostname = normalize_hostname(&stado::providers::vast::system_hostname());
    // The catalog gates join wherever the registry is readable from here;
    // on carry-over setups the control plane gates at approve instead.
    match fetch_document().await {
        Ok(document) => catalog::require_join_allowed(&document)?,
        Err(_) => println!("note: registry not readable here; the catalog gates at approve"),
    }
    release_platform(std::env::consts::OS, std::env::consts::ARCH)?;
    let request = build_request(&hostname, std::env::consts::OS, std::env::consts::ARCH);
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let created = store
        .create_text_if_absent(
            &request_path(&hostname),
            &serde_json::to_string_pretty(&request).map_err(|exc| exc.to_string())?,
        )
        .await
        .map_err(|exc| exc.to_string())?;
    if created {
        println!("join request recorded for '{hostname}'");
    } else {
        println!("a join request for '{hostname}' already exists");
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&request).map_err(|exc| exc.to_string())?
    );
    println!(
        "next step, on the control plane: stado_fleet approve '{}'",
        target_name_for(&hostname)
    );
    Ok(true)
}

/// `stado_fleet pending` — every unanswered join request in the store.
pub async fn pending() -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let blobs = store
        .list_blobs_with_meta(REQUESTS_PREFIX)
        .await
        .map_err(|exc| exc.to_string())?;
    let mut shown = Vec::new();
    for blob in &blobs {
        let Some(text) = store
            .download_text(&blob.name)
            .await
            .map_err(|exc| exc.to_string())?
        else {
            continue;
        };
        let Ok(document) = serde_json::from_str::<Value>(&text) else {
            continue;
        };
        if let Ok(hostname) = pending_request(&document) {
            shown.push(hostname.to_string());
        }
    }
    if shown.is_empty() {
        println!("no pending join requests");
    } else {
        for hostname in &shown {
            println!("{hostname}");
        }
    }
    Ok(true)
}

/// `stado_fleet approve HOSTNAME [--fleet FLEET]` — convert a pending
/// request into a registered target, optionally in one fleet.
pub async fn approve(hostname: &str, fleet_name: Option<&str>) -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let text = store
        .download_text(&request_path(hostname))
        .await
        .map_err(|exc| exc.to_string())?
        .ok_or_else(|| format!("no join request for '{hostname}'"))?;
    let request: Value = serde_json::from_str(&text).map_err(|exc| exc.to_string())?;
    let request_hostname = pending_request(&request)?.to_string();
    let name = target_name_for(&request_hostname);
    let kind = request
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("local")
        .to_string();
    let request_os = request
        .get("os")
        .and_then(Value::as_str)
        .ok_or_else(|| "join request has no operating system".to_string())?;
    let request_arch = request
        .get("arch")
        .and_then(Value::as_str)
        .ok_or_else(|| "join request has no architecture".to_string())?;
    let release_platform = release_platform(request_os, request_arch)?;
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    catalog::require_join_allowed(&document)?;
    let next = register_target(
        &document,
        &name,
        &kind,
        std::slice::from_ref(&request_hostname),
        release_platform,
    )?;
    let generation = push_document(&next).await.map_err(|exc| exc.to_string())?;
    println!("approved '{request_hostname}' as target '{name}' (generation {generation})");
    if let Some(fleet) = fleet_name {
        crate::ops::assign(&name, fleet).await?;
    }
    let mut decided = request;
    decided["status"] = Value::String(STATUS_APPROVED.to_string());
    store
        .upload_text(
            &request_path(hostname),
            &serde_json::to_string_pretty(&decided).map_err(|exc| exc.to_string())?,
        )
        .await
        .map_err(|exc| exc.to_string())?;
    println!("install the agent on the machine: stado bootstrap --local --target '{name}'");
    Ok(true)
}

/// `stado_fleet reject HOSTNAME` — drop a pending join request.
pub async fn reject(hostname: &str) -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    store
        .delete_blob(&request_path(hostname))
        .await
        .map_err(|exc| exc.to_string())?;
    println!("rejected join request for '{hostname}'");
    Ok(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_carries_machine_facts_and_pending_status() {
        let request = build_request("Worker-Box.local", "macos", "aarch64");
        assert_eq!(
            request.get("hostname").and_then(Value::as_str),
            Some("Worker-Box.local")
        );
        assert_eq!(
            request.get("status").and_then(Value::as_str),
            Some(STATUS_PENDING)
        );
        assert_eq!(request.get("arch").and_then(Value::as_str), Some("aarch64"));
    }

    #[test]
    fn target_name_is_the_normalized_hostname() {
        assert_eq!(target_name_for("Worker-Box.LOCAL."), "worker-box.local");
    }

    #[test]
    fn pending_request_accepts_fresh_request() {
        let request = build_request("worker-box.local", "linux", "x86_64");
        assert_eq!(
            pending_request(&request).expect("pending"),
            "worker-box.local"
        );
    }

    #[test]
    fn pending_request_refuses_decided_request() {
        let mut request = build_request("worker-box.local", "linux", "x86_64");
        request["status"] = Value::String(STATUS_APPROVED.to_string());
        let err = pending_request(&request).unwrap_err();
        assert!(err.contains("not 'pending'"), "unexpected error: {err}");
    }

    #[test]
    fn pending_request_refuses_missing_hostname() {
        let request = json!({ "status": "pending" });
        let err = pending_request(&request).unwrap_err();
        assert!(err.contains("no hostname"), "unexpected error: {err}");
    }
}
