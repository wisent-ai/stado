//! The four operator-facing enrollment commands: `join` on the machine being
//! added, then `pending`, `approve` and `reject` on the control plane.

use crate::cli::registry::{commit_document, fetch_document};
use crate::cli::CmdError;
use crate::queue::JobStorage;
use crate::targets::normalize_hostname;
use serde_json::{json, Value};

use crate::cli::fleet::ops::register_target;

use super::catalog;
use super::request::{
    build_request, pending_request, release_platform, request_destination, request_invite_id,
    request_path, request_target_name, target_name_for,
};
use super::request::{REQUESTS_PREFIX, STATUS_APPROVED, STATUS_PENDING};

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
///
/// With `as_json` progress goes to standard error and standard output carries
/// one `{approved, target, generation, fleet, enrollment, invite_spent,
/// install_with}` document.
pub async fn approve(
    hostname: &str,
    fleet_name: Option<&str>,
    as_json: bool,
) -> Result<bool, CmdError> {
    let store = JobStorage::new().await?;
    let text = store
        .download_text(&request_path(hostname))
        .await?
        .ok_or_else(|| CmdError::missing(format!("no join request for '{hostname}'")))?;
    let request: Value = serde_json::from_str(&text).map_err(|exc| {
        CmdError::unreachable(format!(
            "the stored join request for '{hostname}' is not JSON: {exc}"
        ))
    })?;
    let request_hostname = pending_request(&request)
        .map_err(CmdError::refused)?
        .to_string();
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
    let document = fetch_document().await?;
    let mut enrollment = Value::Null;
    let mut registered_generation = Value::Null;
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
            if as_json {
                enrollment = crate::cli::fleet::ops::enrolled(
                    &name,
                    Some(destination),
                    &kind,
                    fleet_name,
                    true,
                    false,
                    true,
                )
                .await?;
                registered_generation = enrollment["generation"].clone();
            } else {
                crate::cli::fleet::ops::enroll(
                    &name,
                    Some(destination),
                    &kind,
                    fleet_name,
                    true,
                    false,
                    false,
                )
                .await?;
            }
        }
        None => {
            catalog::require_join_allowed(&document)?;
            let request_os = request
                .get("os")
                .and_then(Value::as_str)
                .ok_or_else(|| CmdError::refused("join request has no operating system"))?;
            let request_arch = request
                .get("arch")
                .and_then(Value::as_str)
                .ok_or_else(|| CmdError::refused("join request has no architecture"))?;
            let release_platform =
                release_platform(request_os, request_arch).map_err(CmdError::refused)?;
            // Pure: the entry, and the fleet it is placed in, are a function of
            // the document they are written into and of the join request, which
            // is already decided. A lost race is answered by applying both to
            // the newer document, in the one write.
            let generation = commit_document(|document| {
                let registered = register_target(
                    document,
                    &name,
                    &kind,
                    std::slice::from_ref(&request_hostname),
                    release_platform,
                )?;
                match fleet_name {
                    Some(fleet) => crate::cli::fleet::ops::assign_target(&registered, &name, fleet),
                    None => Ok(registered),
                }
            })
            .await?;
            registered_generation = json!(generation);
            if !as_json {
                println!(
                    "approved '{request_hostname}' as target '{name}' (generation {generation})"
                );
                if let Some(fleet) = fleet_name {
                    println!(
                        "target '{name}' assigned to fleet '{fleet}' (generation {generation})"
                    );
                }
            }
        }
    }
    let mut decided = request;
    decided["status"] = Value::String(STATUS_APPROVED.to_string());
    store
        .upload_text(
            &request_path(hostname),
            &serde_json::to_string_pretty(&decided)?,
        )
        .await?;
    // The invite has produced a registered machine; nothing is left for it to
    // do, whatever allowance it had left.
    if let Some(invite_id) = &invite_id {
        crate::cli::fleet::invite::mark_spent(&store, invite_id).await?;
    }
    let install_with = destination
        .is_none()
        .then(|| format!("stado bootstrap --local --target '{name}'"));
    if as_json {
        let answer = json!({
            "approved": request_hostname,
            "target": name,
            "generation": registered_generation,
            "fleet": fleet_name,
            "enrollment": enrollment,
            "invite_spent": invite_id,
            "install_with": install_with,
        });
        crate::cli::print_answer(&answer, true)?;
        return Ok(true);
    }
    if let Some(invite_id) = &invite_id {
        println!("invite {invite_id} is spent");
    }
    if let Some(install_with) = install_with {
        println!("install the agent on the machine: {install_with}");
    }
    Ok(true)
}

/// `stado fleet reject HOSTNAME` — drop a pending join request.
pub async fn reject(hostname: &str, as_json: bool) -> Result<bool, CmdError> {
    let store = JobStorage::new().await?;
    store.delete_blob(&request_path(hostname)).await?;
    if as_json {
        crate::cli::print_answer(&json!({ "rejected": hostname }), true)?;
    } else {
        println!("rejected join request for '{hostname}'");
    }
    Ok(true)
}
