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

    /// Run `stado <arguments>` on the owner host through the host channel and
    /// answer its standard output: for a command that reads items whole,
    /// which only the owner may. A refusal there is refused here with the
    /// owner's own last error line.
    pub(in crate::cli::database) async fn run_there(
        host: &str,
        arguments: &[&str],
    ) -> Result<String, CmdError> {
        let target = crate::cli::canonical_host(host).await?;
        let runner = crate::deploy::production_runner();
        let home = crate::deploy::host_channel::remote_home(&target, &runner)
            .await
            .map_err(CmdError::from)?;
        let stado = format!("{home}/.stado/bin/stado");
        let mut program = vec![stado.as_str()];
        program.extend_from_slice(arguments);
        let output = crate::deploy::host_channel::run_program(&target, &program, &runner)
            .await
            .map_err(CmdError::from)?;
        if !output.ok() {
            let mut refused = CmdError::click(format!(
                "{}: stado {} refused: {}",
                target.name,
                arguments.join(" "),
                crate::deploy::host_channel::last_error_line(&output, "no output")
            ));
            refused.failure = Some(crate::primitives::failure::FailureCode::Refused);
            return Err(refused);
        }
        Ok(output.stdout)
    }
}

/// The same adopt for the owner host, which reads each item whole: a
/// password file is on this machine, so it is refused rather than dropped.
pub(super) fn forwarded(
    name: Option<&str>,
    project_ref: Option<&str>,
    password_file: Option<&str>,
    check: bool,
    json_output: bool,
) -> Result<Vec<String>, CmdError> {
    if password_file.is_some() {
        return Err(CmdError::usage(
            "--password-file names a file on this machine, and adopt runs on the owner vault host; give the password there",
        ));
    }
    let mut arguments = vec!["database".to_string(), "adopt".to_string()];
    arguments.extend(name.map(str::to_string));
    if let Some(reference) = project_ref {
        arguments.extend(["--project-ref".to_string(), reference.to_string()]);
    }
    arguments.extend(check.then(|| "--check".to_string()));
    arguments.extend(json_output.then(|| "--json".to_string()));
    Ok(arguments)
}
