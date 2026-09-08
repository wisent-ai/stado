//! The public half of the fleet's channel key for one invite's target. The
//! private half never leaves the operator's vault, and this route never reads
//! it.

use std::time::Instant;

use serde_json::json;

use crate::dashboard::{http_status, send_json, Request, Response};
use crate::queue::JobStorage;

use super::super::presented;
use super::super::redeem::verify;
use super::super::refusals::{denied, refuse, unavailable};
use super::super::window::accept_request;

/// `GET /api/fleet/invite/key` — the public half of the fleet's channel key
/// for this invite's target, plus the exact `authorized_keys` line to append.
/// Reads no registry, writes nothing, spends nothing.
pub(in crate::dashboard) async fn invite_key(store: &JobStorage, request: &Request) -> Response {
    let started = Instant::now();
    let token = presented(request);
    if !accept_request(token.as_ref().map(|(id, _)| id.as_str()), request.peer) {
        return refuse(started).await;
    }
    let Some((id, secret)) = token else {
        return refuse(started).await;
    };
    if !request.body.is_empty() {
        return refuse(started).await;
    }
    let accepted = match verify(store, &id, &secret).await {
        Ok(accepted) => accepted,
        Err(denial) => return denied(started, denial).await,
    };

    let target = accepted.invite.target_name;
    let item = crate::cli::fleet::key::item_id(&target);
    let client = match crate::cli::fleet::key::configured_client() {
        Ok(client) => client,
        Err(_) => return unavailable("enrollment key store is unavailable"),
    };
    let stored = match client.read_string(&item, "public_key").await {
        Ok(Some(value)) if !value.trim().is_empty() => value,
        Ok(_) => return unavailable("enrollment key is not available for this invite"),
        Err(_) => return unavailable("enrollment key store is unavailable"),
    };
    // `ssh-keygen` leaves its own comment on the key, so the stored value is
    // "<type> <blob> [comment]". The machine appends one line naming the
    // credential item it came from; repeating ssh-keygen's comment there
    // would make that line self-describing twice and match nothing an
    // operator later greps for.
    let mut fields = stored.split_whitespace();
    let public_key = match (fields.next(), fields.next()) {
        (Some(kind), Some(blob)) => format!("{kind} {blob}"),
        _ => return unavailable("enrollment key is not available for this invite"),
    };
    send_json(
        http_status("200"),
        &json!({
            "target_name": target,
            "public_key": public_key,
            "authorized_keys_line":
                crate::cli::fleet::key::authorized_keys_line(&public_key, &item),
        }),
    )
}
