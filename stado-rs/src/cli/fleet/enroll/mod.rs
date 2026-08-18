//! Machine-initiated enrollment: `join`, `pending`, `approve`, `reject`.
//!
//! The machine being added knows everything about itself, so the flow
//! starts there: `stado fleet join` announces the machine's real hostname,
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
use crate::cli::registry::{fetch_document, push_document};
use crate::queue::JobStorage;
use crate::targets::normalize_hostname;

use crate::cli::fleet::ops::register_target;

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

/// Build the request an invited machine files: the same document plus the four
/// facts an invite adds — the name the invite reserved, which channel the fleet
/// should come back on, which invite paid for it, and which key the machine
/// says it installed.
///
/// `target_name` is not cosmetic. The invite minted the channel key as
/// `stado-ssh-<target_name>` and told the machine's owner that name, so
/// approving under the machine's own hostname instead would reach for a key
/// nobody ever minted — the channel would fail after the owner had already
/// installed the right key. `destination` is what makes approval a verified
/// enrollment instead of a declaration, so it is the field `approve` branches
/// on. Pure.
pub fn build_invited_request(
    hostname: &str,
    os: &str,
    arch: &str,
    target_name: &str,
    destination: &str,
    invite_id: &str,
    installed_key_fingerprint: &str,
) -> Value {
    let mut request = build_request(hostname, os, arch);
    request["target_name"] = Value::String(target_name.to_string());
    request["destination"] = Value::String(destination.to_string());
    request["invite_id"] = Value::String(invite_id.to_string());
    request["installed_key_fingerprint"] =
        Value::String(installed_key_fingerprint.to_string());
    request
}

/// The SSH destination an invited machine asked the fleet to come back on, or
/// `None` for today's machine-initiated `join` (which has no channel). Pure.
pub fn request_destination(document: &Value) -> Option<&str> {
    document
        .get("destination")
        .and_then(Value::as_str)
        .filter(|destination| !destination.trim().is_empty())
}

/// The registry name an invite reserved for this machine, if the request came
/// from one. Pure.
pub fn request_target_name(document: &Value) -> Option<&str> {
    document
        .get("target_name")
        .and_then(Value::as_str)
        .filter(|name| !name.trim().is_empty())
}

/// The invite that paid for a request, if any. Pure.
pub fn request_invite_id(document: &Value) -> Option<&str> {
    document
        .get("invite_id")
        .and_then(Value::as_str)
        .filter(|id| !id.is_empty())
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

/// `stado fleet join` — run on the machine being added. Announces itself
/// in the store and prints the request for carry-over setups.
pub async fn join() -> Result<bool, String> {
    let hostname = normalize_hostname(&crate::providers::vast::system_hostname());
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
        "next step, on the control plane: stado fleet approve '{}'",
        target_name_for(&hostname)
    );
    Ok(true)
}

/// One pending request as `pending` reports it: the machine's own facts plus
/// whatever an invite added. Pure.
fn pending_row(document: &Value) -> Value {
    let text = |name: &str| document.get(name).and_then(Value::as_str);
    json!({
        "hostname": text("hostname"),
        "os": text("os"),
        "arch": text("arch"),
        "kind": text("kind").unwrap_or("local"),
        "target_name": request_target_name(document),
        "status": STATUS_PENDING,
        "requested_at": text("requested_at"),
        "destination": request_destination(document),
        "invite_id": request_invite_id(document),
        "installed_key_fingerprint": text("installed_key_fingerprint"),
        "ssh_listening": document.get("ssh_listening").and_then(Value::as_bool),
    })
}

/// `stado fleet pending` — every unanswered join request in the store.
///
/// An invited machine's request carries the channel it asked the fleet to come
/// back on, so the destination is shown: it is the difference between a request
/// `approve` can verify by probing and one it can only take on trust.
pub async fn pending(as_json: bool) -> Result<bool, String> {
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
        if pending_request(&document).is_ok() {
            shown.push(pending_row(&document));
        }
    }
    if as_json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({ "pending": shown }))
                .map_err(|exc| exc.to_string())?
        );
        return Ok(true);
    }
    if shown.is_empty() {
        println!("no pending join requests");
        return Ok(true);
    }
    for row in &shown {
        let text = |name: &str| row.get(name).and_then(Value::as_str).unwrap_or("-");
        println!("{}", text("hostname"));
        if let Some(target) = row.get("target_name").and_then(Value::as_str) {
            println!("  target:   {target} (the name the invite reserved)");
        }
        match row.get("destination").and_then(Value::as_str) {
            Some(destination) => {
                println!("  channel:  {destination} (approve verifies it by probing)")
            }
            None => println!("  channel:  none declared; approve registers without probing"),
        }
        if let Some(invite) = row.get("invite_id").and_then(Value::as_str) {
            println!("  invite:   {invite}");
            println!("  key:      {}", text("installed_key_fingerprint"));
        }
        if row.get("ssh_listening").and_then(Value::as_bool) == Some(false) {
            println!("  warning:  the machine reported that nothing answers on its ssh port yet");
        }
    }
    Ok(true)
}

/// `stado fleet approve HOSTNAME [--fleet FLEET]` — turn a pending request
/// into a registered target.
///
/// Two kinds of request arrive here. One carries a `destination` (an invited
/// machine, which has already installed the fleet's public key): that one takes
/// the ordinary probing [`crate::cli::fleet::ops::enroll`] path verbatim —
/// probe first, write second, roll the entry back if the agent will not
/// install. Approval does not get its own, weaker registration path just
/// because the request came in from outside. The other kind has no channel at
/// all (today's `join`), and is registered from the machine's own report as
/// before.
///
/// The registry name comes from the request, not from this command: an invited
/// machine is registered under the name its invite reserved (and minted the
/// channel key for, and showed its owner), while a plain `join` request is
/// registered under the machine's own hostname as before.
pub async fn approve(hostname: &str, fleet_name: Option<&str>) -> Result<bool, String> {
    let store = JobStorage::new().await.map_err(|exc| exc.to_string())?;
    let text = store
        .download_text(&request_path(hostname))
        .await
        .map_err(|exc| exc.to_string())?
        .ok_or_else(|| format!("no join request for '{hostname}'"))?;
    let request: Value = serde_json::from_str(&text).map_err(|exc| exc.to_string())?;
    let request_hostname = pending_request(&request)?.to_string();
    // An invited request names the target the invite reserved and minted the
    // channel key for; only a request without one falls back to the machine's
    // own hostname.
    let name = request_target_name(&request)
        .map(str::to_string)
        .unwrap_or_else(|| target_name_for(&request_hostname));
    let kind = request
        .get("kind")
        .and_then(Value::as_str)
        .unwrap_or("local")
        .to_string();
    let destination = request_destination(&request).map(str::to_string);
    let invite_id = request_invite_id(&request).map(str::to_string);
    let document = fetch_document().await.map_err(|exc| exc.to_string())?;
    match &destination {
        Some(destination) => {
            if invite_id.is_some() {
                catalog::require_invite_allowed(&document)?;
            } else {
                catalog::require_join_allowed(&document)?;
            }
            // `install_key` is false: an invited machine put the fleet's public
            // key in its own authorized_keys as the invite's first act, so
            // there is nothing to install and no second channel to do it over.
            crate::cli::fleet::ops::enroll(
                &name,
                Some(destination),
                &kind,
                fleet_name,
                true,
                false,
            )
            .await?;
        }
        None => {
            catalog::require_join_allowed(&document)?;
            let request_os = request
                .get("os")
                .and_then(Value::as_str)
                .ok_or_else(|| "join request has no operating system".to_string())?;
            let request_arch = request
                .get("arch")
                .and_then(Value::as_str)
                .ok_or_else(|| "join request has no architecture".to_string())?;
            let release_platform = release_platform(request_os, request_arch)?;
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
                crate::cli::fleet::ops::assign(&name, fleet).await?;
            }
        }
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
    // The invite has produced a registered machine; nothing is left for it to
    // do, whatever allowance it had left.
    if let Some(invite_id) = &invite_id {
        crate::cli::fleet::invite::mark_spent(&store, invite_id).await?;
        println!("invite {invite_id} is spent");
    }
    if destination.is_none() {
        println!("install the agent on the machine: stado bootstrap --local --target '{name}'");
    }
    Ok(true)
}

/// `stado fleet reject HOSTNAME` — drop a pending join request.
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
    fn invited_request_carries_the_reserved_name_channel_and_invite() {
        let request = build_invited_request(
            "worker-box.local",
            "Darwin",
            "arm64",
            "invited-0123abcd",
            "owner@worker-box.local",
            "0123456789abcdef",
            "SHA256:abc",
        );
        assert_eq!(request_target_name(&request), Some("invited-0123abcd"));
        assert_eq!(
            request_destination(&request),
            Some("owner@worker-box.local")
        );
        assert_eq!(request_invite_id(&request), Some("0123456789abcdef"));
        assert_eq!(
            request.get("status").and_then(Value::as_str),
            Some(STATUS_PENDING)
        );
        // The channel key the invite minted is named after the reserved target,
        // so approving under the hostname would reach for a key that was never
        // minted. A row that reports one and registers the other is the bug.
        let row = pending_row(&request);
        assert_eq!(
            row.get("target_name").and_then(Value::as_str),
            Some("invited-0123abcd")
        );
    }

    #[test]
    fn a_plain_join_request_declares_no_channel_and_no_reserved_name() {
        let request = build_request("worker-box.local", "macos", "aarch64");
        assert_eq!(request_destination(&request), None);
        assert_eq!(request_target_name(&request), None);
        assert_eq!(request_invite_id(&request), None);
        let row = pending_row(&request);
        assert!(row.get("destination").expect("key present").is_null());
        assert!(row.get("target_name").expect("key present").is_null());
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
