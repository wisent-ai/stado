//! The database credential item lives in the fleet's owner vault, whichever
//! host runs `stado database create`. The owner host writes it into its own
//! vault; any other host sends the same `set-json` payload to the owner
//! through the host channel (`stado credentials item put --host`), so a
//! database can be created from the operator's machine without a local vault
//! copy standing in for the owner's.

use serde_json::{json, Value};

use crate::cli::CmdError;
use crate::credential_store::owner;

/// The envelope `skarbiec set-json` validates, as `owner::store_json` sends it.
const ITEM_SCHEMA: &str = "skarbiec.item.v2";

/// Where the owner vault is, relative to this host.
pub(in crate::cli::database) enum Owner {
    /// This host owns the vault.
    Here,
    /// The named registry host owns it.
    Host(String),
}

pub(in crate::cli::database) async fn locate() -> Result<Owner, CmdError> {
    let (owner, here) = crate::cli::release_catalog::fleet_hosts().await?;
    Ok(if owner == here {
        Owner::Here
    } else {
        Owner::Host(owner)
    })
}

impl Owner {
    pub(in crate::cli::database) fn name(&self) -> &str {
        match self {
            Self::Here => "this host",
            Self::Host(host) => host,
        }
    }

    /// Store `item` in the owner vault; refused before any write when the
    /// owner cannot be reached, naming the host and the failed step.
    pub(in crate::cli::database) async fn store(
        &self,
        item: &str,
        item_type: &str,
        fields: &Value,
        context: &Value,
    ) -> Result<(), CmdError> {
        match self {
            Self::Here => owner::write_item(item, item_type, fields, context).map_err(|error| {
                CmdError::click(format!(
                    "{item} was not stored in this host's owner vault: {error}"
                ))
            }),
            Self::Host(host) => {
                let payload = json!({
                    "schema": ITEM_SCHEMA,
                    "kind": item_type,
                    "fields": fields,
                    "context": context,
                })
                .to_string();
                crate::cli::host::store_vault_item(host, item, item_type, &payload, false)
                    .await
                    .map_err(|error| {
                        CmdError::click(format!(
                            "{item} was not stored in {host}'s owner vault: {error}"
                        ))
                    })
            }
        }
    }

    /// Whether the owner vault accepts a write now: a created project whose
    /// generated password cannot be stored would be unreachable.
    pub(in crate::cli::database) fn ready(&self) -> Result<(), CmdError> {
        match self {
            Self::Here => owner::vault().map(|_| ()).map_err(|error| {
                CmdError::click(format!(
                    "this host's owner vault cannot be written: {error}"
                ))
            }),
            Self::Host(_) => Ok(()),
        }
    }

    /// The password an existing item already holds, which a rewrite must keep.
    pub(super) async fn password(&self, item: &str) -> Result<String, CmdError> {
        match self {
            Self::Here => owner::read_string(item, "db_password").map_err(|error| {
                CmdError::click(format!(
                    "{item}#db_password could not be read here: {error}"
                ))
                .stating(error.failure_code())
            }),
            Self::Host(host) => crate::credential_store::read_string(item, "db_password")
                .await
                .map_err(|error| {
                    CmdError::click(format!(
                        "{item}#db_password could not be read from {host}: {error}"
                    ))
                    .stating(error.failure_code())
                })?
                .filter(|value| !value.is_empty())
                .ok_or_else(|| {
                    CmdError::click(format!("{item} on {host} holds no db_password"))
                        .stating(crate::primitives::failure::FailureCode::NotFound)
                }),
        }
    }

    /// The whole item document (`fields`, `context`, …), read where the
    /// owner vault is: a rewrite keeps the fields another writer put on it.
    /// `None` when this host's own vault holds no such item.
    pub(super) async fn document(&self, item: &str) -> Result<Option<Value>, CmdError> {
        match self {
            Self::Here => owner::read_document(item).map_err(|error| {
                CmdError::click(format!("{item} could not be read: {error}"))
                    .stating(error.failure_code())
            }),
            Self::Host(host) => crate::cli::host::owner_item_document(host, item)
                .await
                .map(Some)
                .map_err(|error| {
                    let mut refused = CmdError::click(format!(
                        "{item} could not be read on {host}: {}",
                        error.message.as_deref().unwrap_or("no detail")
                    ));
                    refused.failure = error.failure;
                    refused
                }),
        }
    }

    /// The live item ids of a remote owner vault, listed once for a command
    /// that visits many items, so an absent item is reported rather than read
    /// as a failure. `None` here, where each read answers for itself.
    pub(super) async fn item_ids(
        &self,
    ) -> Result<Option<std::collections::BTreeSet<String>>, CmdError> {
        match self {
            Self::Here => Ok(None),
            Self::Host(host) => crate::cli::host::owner_item_ids(host).await.map(Some),
        }
    }
}
