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
