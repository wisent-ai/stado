//! The GitHub identity on a host whose credential store is a file.
//!
//! Skarbiec's route table answers which item and field a route reaches. A
//! file store has no route table, so there the route's resource is the id of
//! the item that holds the credential, and the credential is that item's only
//! string field: `stado credentials put <route> < token` writes exactly that.
//! An item with several string fields names no single credential and is
//! refused with the fields it holds, so nothing is picked by guess.

use std::path::{Path, PathBuf};

use crate::credential_store::Backend;
use crate::deploy::DeployError;
use crate::primitives::failure::FailureCode;

use super::{carrying, ResolvedCredential, DECLARATION_PATH};

/// The file store this host selected, or `None` when it selected Skarbiec.
fn selected_file(route: &str) -> Result<Option<PathBuf>, DeployError> {
    match crate::credential_store::selected() {
        Ok(Backend::File { path }) => Ok(Some(path)),
        Ok(Backend::Skarbiec { .. }) => Ok(None),
        Err(error) => Err(carrying(
            format!(
                "Stado cannot tell which credential store holds the GitHub route {route:?}: {error}"
            ),
            error,
        )),
    }
}

fn refusal(route: &str, path: &Path, detail: &str) -> DeployError {
    DeployError(format!(
        "the credential store file {} holds no GitHub credential for the route {route:?} \
         declared in {DECLARATION_PATH}: {detail}; store it as the only field of the item \
         {route:?} with `stado credentials put {route} < <token-file>`",
        path.display()
    ))
    .stating(FailureCode::NotFound)
}

/// The coordinate the declared route reaches in the selected file store, or
/// `None` when the selected store is Skarbiec, whose route table answers it.
pub(super) async fn resolve(route: &str) -> Result<Option<ResolvedCredential>, DeployError> {
    let Some(path) = selected_file(route)? else {
        return Ok(None);
    };
    let item = crate::credential_store::read_item(route)
        .await
        .map_err(|error| refusal(route, &path, &error.to_string()))?;
    let Some(fields) = item.as_object() else {
        return Err(refusal(route, &path, "the item is not an object of fields"));
    };
    let named: Vec<&String> = fields
        .iter()
        .filter(|(_, value)| value.is_string())
        .map(|(name, _)| name)
        .collect();
    match named.as_slice() {
        [field] => Ok(Some(ResolvedCredential {
            route: route.to_string(),
            item: route.to_string(),
            field: field.to_string(),
            declared_by: DECLARATION_PATH.to_string(),
        })),
        _ => Err(refusal(
            route,
            &path,
            &format!(
                "the item holds {} string fields ({}), not one",
                named.len(),
                named
                    .iter()
                    .map(|name| name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        )),
    }
}

/// The value of a coordinate [`resolve`] answered from the file store, or
/// `None` when the selected store is Skarbiec.
pub(super) async fn read(resolved: &ResolvedCredential) -> Result<Option<String>, DeployError> {
    if selected_file(&resolved.route)?.is_none() {
        return Ok(None);
    }
    let coordinate = format!("{}.{}", resolved.item, resolved.field);
    crate::credential_store::read_declared_string(&resolved.item, &resolved.field)
        .await
        .map_err(|error| {
            carrying(
                format!("the credential store file did not hand over {coordinate}: {error}"),
                error,
            )
        })?
        .filter(|value| !value.trim().is_empty())
        .map(Some)
        .ok_or_else(|| {
            DeployError(format!(
                "the credential the GitHub route {:?} names, {coordinate}, is empty in the \
                 credential store file",
                resolved.route
            ))
            .stating(FailureCode::NotFound)
        })
}
