use crate::cli::{registry, CmdError};
use crate::release_pipeline::destinations::{self, FIELD};
use serde_json::{json, Value};

pub(super) fn put(document: &Value, product: &str, targets: &[String]) -> Result<Value, CmdError> {
    destinations::validate_product(product).map_err(CmdError::usage)?;
    destinations::declarations(document).map_err(CmdError::declaration)?;
    let mut next = document.clone();
    if next.get(FIELD).is_none() {
        next[FIELD] = json!({});
    }
    next[FIELD][product] = json!(targets);
    destinations::validate(&next).map_err(CmdError::declaration)?;
    Ok(next)
}

pub(super) async fn set(product: &str, targets: &[String]) -> Result<String, CmdError> {
    registry::commit_document(|document| put(document, product, targets)).await
}

pub(super) async fn remove(product: &str) -> Result<String, CmdError> {
    destinations::validate_product(product).map_err(CmdError::usage)?;
    registry::commit_document(|document| {
        destinations::declarations(document).map_err(CmdError::declaration)?;
        let mut next = document.clone();
        if let Some(products) = next.get_mut(FIELD).and_then(Value::as_object_mut) {
            products.remove(product);
        }
        Ok(next)
    })
    .await
}

/// One product's destinations, and whether they are its declared list
/// (`declared`) or, with none declared, the hosts whose `managed_versions`
/// say they run it (`running`).
pub(super) async fn show(product: &str) -> Result<Value, CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    let targets = destinations::read(&document, product).map_err(CmdError::declaration)?;
    let declared = destinations::declared(&document, product).map_err(CmdError::declaration)?;
    let state = if declared { "declared" } else { "running" };
    Ok(
        json!({"product": product, "state": state, "registry_generation": generation, "destinations": targets}),
    )
}
