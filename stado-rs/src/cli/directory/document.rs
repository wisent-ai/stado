//! Reads of the `service_directory` block out of the raw registry document,
//! and the writer that mutates one service entry under the block's generation.
//!
//! Each accessor refuses by naming what is absent, because the block is read
//! straight out of the document and an absent key is a fact about the registry
//! rather than a mistake by the caller.

use serde_json::{Map, Value};

use crate::targets;

use crate::cli::registry;
use crate::cli::CmdError;

pub(super) const DIRECTORY_KEY: &str = "service_directory";

pub(super) fn click(message: impl std::fmt::Display) -> CmdError {
    CmdError::click(message.to_string())
}

/// The directory block, or a refusal naming what is absent. A missing block is
/// distinguished from an empty one: the first means nobody has ever declared
/// anything, the second that everything was withdrawn.
pub(super) fn directory(document: &Value) -> Result<&Map<String, Value>, CmdError> {
    document
        .get(DIRECTORY_KEY)
        .and_then(Value::as_object)
        .ok_or_else(|| {
            click(format!(
                "the registry at {} carries no {DIRECTORY_KEY}",
                targets::registry_location()
            ))
        })
}

pub(super) fn services(block: &Map<String, Value>) -> Result<&Map<String, Value>, CmdError> {
    block
        .get("services")
        .and_then(Value::as_object)
        .ok_or_else(|| click(format!("{DIRECTORY_KEY} carries no services map")))
}

pub(super) fn service<'a>(
    block: &'a Map<String, Value>,
    name: &str,
) -> Result<&'a Value, CmdError> {
    let all = services(block)?;
    all.get(name).ok_or_else(|| {
        let known: Vec<&str> = all.keys().map(String::as_str).collect();
        click(format!(
            "no service {name:?} in {DIRECTORY_KEY}; it declares {}",
            known.join(", ")
        ))
    })
}

/// The registry document for a verb that only reads the directory: the
/// authority first, then this host's last-known-good copy, announced on
/// stderr with its age and the authority's own refusal.
///
/// Read-only directory consumers can use retained routes when the authority
/// is unavailable. Writers keep `registry::fetch_document`: a mutation must
/// be checked against the authority's current generation, not a cached copy.
pub(super) async fn read_document() -> Result<Value, CmdError> {
    let authority = match registry::fetch_document().await {
        Ok(document) => return Ok(document),
        Err(error) => error,
    };
    let cause = authority.to_string();
    let Some((_, Some(copy))) = targets::last_good_after(&cause) else {
        return Err(authority);
    };
    let document = crate::cli::resolver::last_good_document()
        .map_err(|cache| click(format!("{cause}; recovery registry failed ({cache})")))?;
    eprintln!("{}", copy.notice);
    Ok(document)
}

/// This machine's fleet name. The directory keys endpoints by target name, not
/// by hostname, so a hostname comparison would miss on every host whose fleet
/// name differs from its own idea of itself.
pub(super) async fn this_target() -> Result<String, CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let registry = registry::read_registry()
        .await
        .map_err(|exc| click(format!("cannot resolve this target: {exc}")))?;
    registry
        .lookup_self(&hostname)
        .map_err(|exc| click(exc.to_string()))?
        .map(|found| found.name.clone())
        .ok_or_else(|| {
            click(format!(
                "host {hostname} is not in {}",
                targets::registry_location()
            ))
        })
}

/// This machine's fleet name, read from a document the caller already holds.
///
/// `service directory connect` read the registry once for the directory and
/// then twice more through `this_target`, and with the object API refusing,
/// each read spends its retries before falling back to the copy. Three
/// sequential reads outlast the limit of every agent hook that asks for
/// Brama's address, so every hooked tool call is refused. One document
/// answers all three questions, from one generation.
pub(super) fn this_target_in(document: &Value) -> Result<String, CmdError> {
    let hostname = crate::providers::vast::system_hostname();
    let registry = targets::load_registry_from_value(document)
        .map_err(|exc| click(format!("cannot resolve this target: {exc}")))?;
    registry
        .lookup_self(&hostname)
        .map_err(|exc| click(exc.to_string()))?
        .map(|found| found.name.clone())
        .ok_or_else(|| {
            click(format!(
                "host {hostname} is not in {}",
                targets::registry_location()
            ))
        })
}

/// The address the directory hands one target for one service.
///
/// One spelling, shared by the write loop and by the sweep that decides what
/// is a fossil, because "declared for this host" answered two ways is how a
/// marker gets written by one half of this function and deleted by the other.
pub(super) fn endpoint_url<'a>(entry: &'a Value, target: &str) -> Option<&'a str> {
    entry
        .get("endpoints")
        .and_then(Value::as_object)
        .and_then(|endpoints| endpoints.get(target))
        .and_then(|endpoint| endpoint.get("url"))
        .and_then(Value::as_str)
}
