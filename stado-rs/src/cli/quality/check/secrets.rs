//! The secret environment a release recipe declares, resolved for a check.
//!
//! The release worker puts each `secret_env` value into the build's
//! environment before a gate runs; a gate that needs one (a web install whose
//! lockfile pins private GitHub dependencies reads `GITHUB_TOKEN`) found
//! nothing when the same gate ran at push, and refused a product that had
//! declared exactly what it needs. A check therefore reads each declared
//! `role#field` from the operator's credential store by role, the read
//! `stado credentials get --role` makes, and refuses naming the variable and
//! role when it cannot.

use std::collections::BTreeMap;

use crate::cli::secrets::{item_in_role, store, Store};
use crate::cli::CmdError;

/// Each declared `NAME` with the value its `role#field` holds now.
pub(super) async fn resolved(
    declared: &BTreeMap<String, String>,
) -> Result<Vec<(String, String)>, CmdError> {
    if declared.is_empty() {
        return Ok(Vec::new());
    }
    let store = store()?;
    let mut resolved = Vec::with_capacity(declared.len());
    for (name, reference) in declared {
        let (role, field) = reference.split_once('#').ok_or_else(|| {
            CmdError::declaration(format!(
                "secret_env {name} is {reference:?}, not a role#field reference"
            ))
        })?;
        let value = role_field(&store, role, field).await.map_err(|error| {
            error.within(format!(
                "secret_env {name} ({reference}) cannot be read for the quality gates"
            ))
        })?;
        resolved.push((name.clone(), value));
    }
    Ok(resolved)
}

async fn role_field(store: &Store, role: &str, field: &str) -> Result<String, CmdError> {
    let item = item_in_role(store, role).await?;
    match store {
        Store::Skarbiec(vault) => vault.read_declared_string(&item, field).await,
        Store::File(_) => crate::credential_store::read_string(&item, field).await,
    }
    .map_err(CmdError::from)?
    .filter(|value| !value.is_empty())
    .ok_or_else(|| {
        CmdError::click(format!(
            "the item playing role {role:?} ({item:?}) has no non-empty string field {field:?}"
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound)
    })
}
