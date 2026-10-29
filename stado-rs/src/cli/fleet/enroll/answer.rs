//! Answering a join request on the control plane: `approve` turns it into a
//! registered target, `reject` drops it.
//!
//! Every way `approve` fails is a declared cause of `fleet.approve`
//! ([`super::failures::APPROVE`]), raised at the step that broke.

use crate::cli::registry::{commit_document, fetch_document};
use crate::cli::CmdError;
use crate::queue::JobStorage;
use serde_json::{json, Value};

use crate::cli::fleet::ops::{register_target, require_declared_fleet};

use super::catalog;
use super::failures::{
    write_failure, FLEETS_MALFORMED, INVITE_UNSPENT, NAME_INVALID, PLATFORM_UNSUPPORTED,
    REGISTRY_READ, REQUEST_DECIDED, REQUEST_INCOMPLETE, REQUEST_MISSING, REQUEST_STATUS_UNWRITTEN,
    REQUEST_STORE, REQUEST_UNREADABLE,
};
use super::request::{
    release_platform, request_destination, request_invite_id, request_path, request_target_name,
    target_name_for, REQUESTS_PREFIX, STATUS_APPROVED, STATUS_PENDING,
};

/// The hostnames of the requests the store holds, for a refusal that names
/// what can be approved.
async fn pending_names(store: &JobStorage) -> String {
    match store.list_blobs_with_meta(REQUESTS_PREFIX).await {
        Ok(blobs) => {
            let names: Vec<&str> = blobs
                .iter()
                .filter_map(|blob| {
                    blob.name
                        .strip_prefix(REQUESTS_PREFIX)
                        .and_then(|name| name.strip_suffix(".json"))
                        .filter(|name| !name.contains('/'))
                })
                .collect();
            if names.is_empty() {
                "none".to_string()
            } else {
                names.join(", ")
            }
        }
        Err(error) => format!("the requests could not be listed ({error})"),
    }
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
    // The hostname names the stored request's path, so a name of any other
    // shape is refused before the store is asked.
    if !crate::targets::is_target_name(hostname) {
        return Err(CmdError::declared(
            &NAME_INVALID,
            &[
                ("name", hostname),
                ("suggestion", &target_name_for(hostname)),
            ],
        ));
    }
    let values = [("hostname", hostname), ("name", hostname)];
    let unreachable = |error: CmdError| REQUEST_STORE.raise(error, &values);
    let store = JobStorage::new()
        .await
        .map_err(|error| unreachable(CmdError::from(error)))?;
    let Some(text) = store
        .download_text(&request_path(hostname))
        .await
        .map_err(|error| unreachable(CmdError::from(error)))?
    else {
        let pending = pending_names(&store).await;
        return Err(CmdError::declared(
            &REQUEST_MISSING,
            &[("hostname", hostname), ("pending", &pending)],
        ));
    };
    let request: Value = serde_json::from_str(&text).map_err(|exc| {
        CmdError::declared(
            &REQUEST_UNREADABLE,
            &[("hostname", hostname), ("answer", &exc.to_string())],
        )
    })?;
    let incomplete = |field: &str| {
        CmdError::declared(
            &REQUEST_INCOMPLETE,
            &[
                ("hostname", hostname),
                ("field", field),
                ("request", &request.to_string()),
            ],
        )
    };
    match request.get("status").and_then(Value::as_str) {
        None => return Err(incomplete("status")),
        Some(status) if status != STATUS_PENDING => {
            return Err(CmdError::declared(
                &REQUEST_DECIDED,
                &[("hostname", hostname), ("status", status)],
            ))
        }
        Some(_) => {}
    }
    let request_hostname = request
        .get("hostname")
        .and_then(Value::as_str)
        .filter(|hostname| !hostname.is_empty())
        .ok_or_else(|| incomplete("hostname"))?
        .to_string();
    // An invited request names the target the invite reserved and minted the
    // channel key for; only a request without one takes the machine's own
    // hostname. A request that states no kind came from the local joiner,
    // which is the one kind that does not state it.
    let name = match request_target_name(&request) {
        Some(reserved) => reserved.to_string(),
        None => target_name_for(&request_hostname),
    };
    let kind = match request.get("kind").and_then(Value::as_str) {
        Some(kind) => kind.to_string(),
        None => crate::capabilities::ProviderId::Local.as_str().to_string(),
    };
    let destination = request_destination(&request).map(str::to_string);
    let invite_id = request_invite_id(&request).map(str::to_string);
    let named = [("hostname", hostname), ("name", name.as_str())];
    let document = fetch_document()
        .await
        .map_err(|error| REGISTRY_READ.raise(error, &named))?;
    let mut enrollment = Value::Null;
    let mut registered_generation = Value::Null;
    match &destination {
        Some(destination) => {
            if invite_id.is_some() {
                catalog::require_invited_approval(&document, &name, hostname)?;
            } else {
                catalog::require_join_allowed(&document, &name)?;
            }
            // `install_key` is false: an invited machine put the fleet's public
            // key in its own authorized_keys as the invite's first act, so
            // there is nothing to install and no second channel to do it over.
            let enroll = crate::cli::fleet::ops::EnrollRequest {
                name: &name,
                ssh: destination,
                kind: &kind,
                fleet: fleet_name,
                bootstrap: true,
                install_key: false,
                shared: false,
            };
            if as_json {
                enrollment = crate::cli::fleet::ops::enrolled(enroll, true).await?;
                registered_generation = enrollment["generation"].clone();
            } else {
                crate::cli::fleet::ops::enroll(enroll, false).await?;
            }
        }
        None => {
            catalog::require_join_allowed(&document, &name)?;
            let request_os = request
                .get("os")
                .and_then(Value::as_str)
                .ok_or_else(|| incomplete("operating system (os)"))?;
            let request_arch = request
                .get("arch")
                .and_then(Value::as_str)
                .ok_or_else(|| incomplete("architecture (arch)"))?;
            let release_platform = release_platform(request_os, request_arch).map_err(|_| {
                CmdError::declared(
                    &PLATFORM_UNSUPPORTED,
                    &[("name", &name), ("os", request_os), ("arch", request_arch)],
                )
            })?;
            // The fleet is checked against the document read here before
            // anything is written, so a fleet nobody declared refuses the
            // approval rather than a registration half done.
            if let Some(fleet) = fleet_name {
                require_declared_fleet(&document, &name, fleet)?;
            }
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
                    Some(fleet) => crate::cli::fleet::ops::assign_target(&registered, &name, fleet)
                        .map_err(|error| {
                            CmdError::declared(&FLEETS_MALFORMED, &[("answer", &error.to_string())])
                        }),
                    None => Ok(registered),
                }
            })
            .await
            .map_err(|error| write_failure(error, &named))?;
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
    let unwritten = |answer: String| {
        CmdError::declared(
            &REQUEST_STATUS_UNWRITTEN,
            &[("hostname", hostname), ("name", &name), ("answer", &answer)],
        )
    };
    let decided_text =
        serde_json::to_string_pretty(&decided).map_err(|error| unwritten(error.to_string()))?;
    store
        .upload_text(&request_path(hostname), &decided_text)
        .await
        .map_err(|error| unwritten(error.to_string()))?;
    // The invite has produced a registered machine; nothing is left for it to
    // do, whatever allowance it had left.
    if let Some(invite_id) = &invite_id {
        crate::cli::fleet::invite::mark_spent(&store, invite_id)
            .await
            .map_err(|error| {
                CmdError::declared(
                    &INVITE_UNSPENT,
                    &[
                        ("invite", invite_id),
                        ("name", &name),
                        ("answer", &error.to_string()),
                    ],
                )
            })?;
    }
    // The agent first, then the host's own network, which makes the Stado
    // network its preferred path before any outside overlay.
    let install_with = destination.is_none().then(|| {
        format!("stado bootstrap --local --target '{name}' && stado network up --host '{name}'")
    });
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
        println!(
            "install the agent on the machine and join it to the Stado network: {install_with}"
        );
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
