//! Answering a join request on the control plane: `approve` turns it into a
//! registered target, `reject` drops it.

use crate::cli::registry::{commit_document, fetch_document};
use crate::cli::CmdError;
use crate::queue::JobStorage;
use serde_json::{json, Value};

use crate::cli::fleet::ops::register_target;

use super::catalog;
use super::request::{
    pending_request, release_platform, request_destination, request_invite_id, request_path,
    request_target_name, target_name_for, STATUS_APPROVED,
};

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
