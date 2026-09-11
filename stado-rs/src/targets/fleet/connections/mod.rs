//! The fleet's declared vocabulary of host connection networks.
//!
//! A connection path's name identifies the network underneath an ordinary SSH
//! destination, and the product used to name those networks in places that
//! could disagree: the doc comment on [`super::SshConnectionPath`], the
//! `<PATH>` help of `stado registry host path set`, and whatever an operator
//! remembered. The list is now one document,
//! `stado-rs/data/fleet/connections.json`, compiled into the binary and read
//! by the CLI listing, by Stado Desktop through that listing, and by the
//! connection tests that pick a network to move a host onto.
//!
//! The vocabulary is what the product can describe, not a closed set: any
//! fleet-specific path name is still accepted, because a fleet may run a
//! network Stado has never heard of. What the declaration buys is that every
//! surface offering a choice offers the same one.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

/// The one document declaring every connection network the product names.
pub const CONNECTION_DECLARATION_PATH: &str = "stado-rs/data/fleet/connections.json";
const DECLARATION: &str = include_str!("../../../../data/fleet/connections.json");
/// The shape of the declaration this build reads.
const DECLARATION_SCHEMA_VERSION: u64 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionDeclaration {
    schema_version: u64,
    connection_providers: Vec<ConnectionProvider>,
}

/// One declared network a connection path may name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConnectionProvider {
    /// The path name an operator writes, `tailscale` or `nebula`.
    pub name: String,
    /// What that network is, in the words the CLI and Desktop both show.
    pub summary: String,
}

static DECLARED_PROVIDERS: LazyLock<Result<Vec<ConnectionProvider>, String>> =
    LazyLock::new(|| parse_connection_declaration(DECLARATION));

fn parse_connection_declaration(text: &str) -> Result<Vec<ConnectionProvider>, String> {
    let declaration: ConnectionDeclaration = serde_json::from_str(text).map_err(|error| {
        format!("{CONNECTION_DECLARATION_PATH} is not a valid declaration: {error}")
    })?;
    if declaration.schema_version != DECLARATION_SCHEMA_VERSION {
        return Err(format!(
            "{CONNECTION_DECLARATION_PATH} declares schema_version {}, and this build reads {DECLARATION_SCHEMA_VERSION}",
            declaration.schema_version
        ));
    }
    if declaration.connection_providers.is_empty() {
        return Err(format!(
            "{CONNECTION_DECLARATION_PATH} declares no connection networks"
        ));
    }
    for provider in &declaration.connection_providers {
        if provider.name.trim().is_empty() || provider.summary.trim().is_empty() {
            return Err(format!(
                "{CONNECTION_DECLARATION_PATH} declares a network without a name or a summary"
            ));
        }
    }
    Ok(declaration.connection_providers)
}

/// Every network the product can describe, in declaration order.
///
/// A malformed declaration is a build fault, not a runtime choice: the reader
/// panics rather than hand a caller a silently shorter vocabulary, the same
/// way the reclamation stage declaration refuses to resolve a stage list it
/// could not parse.
pub fn declared_connection_providers() -> &'static [ConnectionProvider] {
    match &*DECLARED_PROVIDERS {
        Ok(providers) => providers,
        Err(error) => panic!("{error}"),
    }
}

/// Whether the product declares this network by name. A path name outside the
/// vocabulary is legitimate — the answer tells an operator which names Stado
/// can describe, and never refuses a route.
pub fn connection_provider_declared(name: &str) -> bool {
    declared_connection_providers()
        .iter()
        .any(|provider| provider.name == name)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The compiled document parses, and the primary path the registry writer
    /// treats specially is one of the networks the product describes.
    #[test]
    fn the_declaration_parses_and_names_the_primary_path() {
        let providers = declared_connection_providers();
        assert!(connection_provider_declared(
            crate::targets::PRIMARY_SSH_CONNECTION
        ));
        assert!(providers
            .iter()
            .all(|provider| !provider.summary.trim().is_empty()));
    }

    /// A declaration this build cannot read is named, with the document that
    /// carries it, instead of becoming an empty vocabulary.
    #[test]
    fn an_unreadable_declaration_says_which_document_it_is() {
        let wrong_shape = parse_connection_declaration("{\"schema_version\": 99}")
            .expect_err("a document of another shape must not parse");
        assert!(
            wrong_shape.contains(CONNECTION_DECLARATION_PATH),
            "{wrong_shape}"
        );
    }
}
