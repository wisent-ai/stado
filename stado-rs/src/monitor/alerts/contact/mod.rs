//! The operator's contact preferences: the alert channels he chose to be
//! reached through, in the order he chose them.
//!
//! The choice is his and the fleet's, not a host's. It used to be each host's
//! `alerts.channels` setting, so the worker that asks for a phone approval
//! paged through whatever its own config file happened to say (one host SMS
//! through an unprovisioned `most`, another e-mail), and nothing anywhere said
//! how the operator wants to be asked. It is one registry field now,
//! `operator_contact.channels`, written by `stado alerts preferences set` and
//! read by every page. No channel is assumed: with no choice recorded, a page
//! is refused with the command that records one.

use serde_json::{json, Value};

use crate::cli::registry::{commit_document, fetch_document};
use crate::cli::CmdError;
use crate::primitives::failure::FailureCode;

/// The registry field that holds the operator's choice.
pub const OPERATOR_CONTACT_KEY: &str = "operator_contact";

/// Every channel Stado can page through, as the operator names them.
pub const CHANNELS: &[&str] = &[
    "slack",
    "telegram",
    "sendgrid",
    "resend",
    "most",
    "gcp-pubsub",
];

/// The channels the operator chose, in his order, read from one registry
/// document. A refusal names what is missing or malformed and how to record
/// the choice.
pub fn chosen_in(document: &Value) -> Result<Vec<String>, String> {
    let record = |problem: String| {
        format!(
            "{problem}; `stado alerts preferences set --channel <name> [--channel <name> ...]` \
             records how the operator wants to be asked (channels: {})",
            CHANNELS.join(", ")
        )
    };
    let contact = document.get(OPERATOR_CONTACT_KEY).ok_or_else(|| {
        record(
            "the operator has chosen no contact channel: the registry holds no operator_contact"
                .into(),
        )
    })?;
    let channels = contact
        .get("channels")
        .and_then(Value::as_array)
        .ok_or_else(|| record("the registry's operator_contact has no channels list".into()))?;
    let mut chosen = Vec::with_capacity(channels.len());
    for value in channels {
        let name = value.as_str().ok_or_else(|| {
            record(format!("the registry's operator_contact.channels holds {value}, which is not a channel name"))
        })?;
        if !CHANNELS.contains(&name) {
            return Err(record(format!(
                "the registry's operator_contact.channels names {name:?}, a channel Stado cannot page through"
            )));
        }
        chosen.push(name.to_string());
    }
    if chosen.is_empty() {
        return Err(record(
            "the registry's operator_contact.channels is empty".into(),
        ));
    }
    Ok(chosen)
}

/// The channels the operator chose, read from the canonical registry.
pub async fn chosen() -> Result<Vec<String>, String> {
    let document = fetch_document().await.map_err(|error| {
        format!(
            "the registry that holds the operator's contact preferences could not be read: {error}"
        )
    })?;
    chosen_in(&document)
}

/// Record the operator's choice: these channels, in this order, replace the
/// previous ones. Every name is checked before anything is written.
pub async fn choose(channels: &[String]) -> Result<String, CmdError> {
    if channels.is_empty() {
        return Err(CmdError::usage(
            "alerts preferences set needs at least one --channel <name>".to_string(),
        ));
    }
    let mut seen: Vec<&str> = Vec::with_capacity(channels.len());
    for name in channels {
        if !CHANNELS.contains(&name.as_str()) {
            return Err(CmdError::usage(format!(
                "{name:?} is not a channel Stado pages through; choose from {}",
                CHANNELS.join(", ")
            )));
        }
        if seen.contains(&name.as_str()) {
            return Err(CmdError::usage(format!("--channel {name} is named twice")));
        }
        seen.push(name);
    }
    commit_document(|document| {
        let mut next = document.clone();
        let object = next.as_object_mut().ok_or_else(|| {
            CmdError::click("the registry document is not an object".to_string())
                .stating(FailureCode::Config)
        })?;
        object.insert(
            OPERATOR_CONTACT_KEY.to_string(),
            json!({ "channels": channels }),
        );
        Ok(next)
    })
    .await
}
