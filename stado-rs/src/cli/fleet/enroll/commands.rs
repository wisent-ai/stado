//! The enrollment commands a joining machine and the queue start from: `join`
//! on the machine being added, and `pending` on the control plane. `approve`
//! and `reject` answer a request (`answer`).

use crate::cli::registry::fetch_document;
use crate::cli::CmdError;
use crate::queue::JobStorage;
use crate::targets::normalize_hostname;
use serde_json::{json, Value};

use super::catalog;
use super::request::{
    build_request, pending_request, release_platform, request_destination, request_invite_id,
    request_path, request_target_name, REQUESTS_PREFIX, STATUS_PENDING,
};

/// `stado fleet join` — run on the machine being added. Announces itself
/// in the store and prints the request for carry-over setups: `key: value`
/// lines, or with `--json` one `{recorded, request, approve_with}` document.
pub async fn join(as_json: bool) -> Result<bool, CmdError> {
    let hostname = normalize_hostname(&crate::providers::vast::system_hostname());
    // The catalog gates join wherever the registry is readable from here;
    // on carry-over setups the control plane gates at approve instead.
    match fetch_document().await {
        Ok(document) => catalog::require_join_allowed(&document)?,
        Err(error) => {
            eprintln!("note: registry not readable here ({error}); the catalog gates at approve")
        }
    }
    release_platform(std::env::consts::OS, std::env::consts::ARCH).map_err(CmdError::refused)?;
    let request = build_request(&hostname, std::env::consts::OS, std::env::consts::ARCH);
    let store = JobStorage::new().await?;
    let created = store
        .create_text_if_absent(
            &request_path(&hostname),
            &serde_json::to_string_pretty(&request)?,
        )
        .await?;
    let approve_with = format!("stado fleet approve '{}'", target_name_for(&hostname));
    if as_json {
        crate::cli::print_answer(
            &json!({ "recorded": created, "request": request, "approve_with": approve_with }),
            true,
        )?;
        return Ok(true);
    }
    if created {
        println!("join request recorded for '{hostname}'");
    } else {
        println!("a join request for '{hostname}' already exists");
    }
    crate::cli::print_answer(&request, false)?;
    println!("next step, on the control plane: {approve_with}");
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
pub async fn pending(as_json: bool) -> Result<bool, CmdError> {
    let store = JobStorage::new().await?;
    let blobs = store.list_blobs_with_meta(REQUESTS_PREFIX).await?;
    let mut shown = Vec::new();
    for blob in &blobs {
        let Some(text) = store.download_text(&blob.name).await? else {
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
            serde_json::to_string_pretty(&json!({ "pending": shown }))?
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
