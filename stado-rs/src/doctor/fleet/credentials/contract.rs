//! Which read contract the credential broker is enforcing.

use serde_json::json;

use crate::doctor::{Check, Status};

pub(in crate::doctor) const CONTRACT_ID: &str = "skarbiec-contract";
pub(in crate::doctor) const CONTRACT_TITLE: &str = "Skarbiec read contract";
pub(in crate::doctor) const CONTRACT_REMEDY: &str =
    "read one field at a time with Client::read_field; a broker that answers without a named field is the thing to fix, not the one that asks for it";

/// Which read contract is the broker enforcing?
///
/// Skarbiec makes `field` mandatory on `/v1/items/read`, and that is correct:
/// a read grant is per field, so answering an item-wide read would hand back
/// fields the caller was never granted. This check used to report that as the
/// fault and a broker answering without a field as healthy, which is exactly
/// backwards -- and a check that calls the secure behaviour a failure teaches
/// operators to skim past `doctor`, which is how a real FAIL sat unread here
/// for hours.
///
/// The drift it exists for is real: callers that ask for a whole item and
/// pick fields out of it get `400 {"error":"field required"}` with no hint
/// the contract has moved, and a host-health beacon stays down for most of
/// a day while `stado service list` reports a
/// stale `active` for services that are not running. The repair is to move
/// those callers to per-field reads, which the remedy now says.
///
/// The probe is unauthenticated on purpose: the handler validates `id` and
/// `field` before it looks at any identity, so a request carrying neither a
/// consumer nor a bearer still reveals which contract is in force, and reveals
/// nothing else.
pub(in crate::doctor) async fn skarbiec_contract_check() -> Check {
    let url = match crate::credential_store::skarbiec_url() {
        Some(url) => url,
        // A file-backed credential store has no broker and no contract to
        // disagree with, which is a different thing from a broker that is
        // fine.
        None => {
            return Check::pass(
                CONTRACT_ID,
                CONTRACT_TITLE,
                "credential store is not a Skarbiec broker; no read contract applies".to_string(),
                CONTRACT_REMEDY,
            )
        }
    };
    let endpoint = format!("{}/v1/items/read", url.trim_end_matches('/'));
    let client = match reqwest::Client::builder().build() {
        Ok(client) => client,
        Err(err) => {
            return Check::new(
                CONTRACT_ID,
                CONTRACT_TITLE,
                Status::Warn,
                format!("could not build an HTTP client to probe {endpoint}: {err}"),
                CONTRACT_REMEDY,
            )
        }
    };
    let response = client
        .post(&endpoint)
        .json(&json!({"id": "stado-doctor-contract-probe"}))
        .send()
        .await;
    match response {
        Err(err) => Check::new(
            CONTRACT_ID,
            CONTRACT_TITLE,
            Status::Warn,
            format!("{endpoint} is unreachable, so the read contract is unknown: {err}"),
            CONTRACT_REMEDY,
        ),
        Ok(response) => {
            let status = response.status();
            // A read that names no field is rejected as an invalid request
            // (400 or 422) by a broker that grants per field; one it answers
            // hands back fields nobody was granted. The status says which,
            // not the body's words.
            if status == reqwest::StatusCode::BAD_REQUEST
                || status == reqwest::StatusCode::UNPROCESSABLE_ENTITY
            {
                Check::pass(
                    CONTRACT_ID,
                    CONTRACT_TITLE,
                    format!(
                        "{endpoint} refused a read that named no field (HTTP {}), which is the \
                         contract in force: the authority grants read per field, so an \
                         item-wide read would hand back fields nobody was granted",
                        status.as_u16()
                    ),
                    CONTRACT_REMEDY,
                )
            } else {
                Check::new(
                    CONTRACT_ID,
                    CONTRACT_TITLE,
                    Status::Warn,
                    format!(
                        "{endpoint} answered a read that named no field (HTTP {}); read \
                         grants are per field, so something here can return fields the caller \
                         was never granted",
                        status.as_u16()
                    ),
                    CONTRACT_REMEDY,
                )
            }
        }
    }
}
