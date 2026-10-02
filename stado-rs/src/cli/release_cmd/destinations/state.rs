use serde_json::{json, Value};
use crate::cli::{registry, CmdError};
use crate::release_pipeline::destinations::{self, FIELD};

pub(super) fn put(document: &Value, product: &str, targets: &[String]) -> Result<Value, CmdError> {
    destinations::validate_product(product).map_err(CmdError::click)?;
    destinations::declarations(document).map_err(CmdError::click)?;
    let mut next = document.clone();
    if next.get(FIELD).is_none() {
        next[FIELD] = json!({});
    }
    next[FIELD][product] = json!(targets);
    destinations::validate(&next).map_err(CmdError::click)?;
    Ok(next)
}

pub(super) async fn set(product: &str, targets: &[String]) -> Result<String, CmdError> {
    registry::commit_document(|document| put(document, product, targets)).await
}

pub(super) async fn remove(product: &str) -> Result<String, CmdError> {
    destinations::validate_product(product).map_err(CmdError::click)?;
    registry::commit_document(|document| {
        destinations::declarations(document).map_err(CmdError::click)?;
        let mut next = document.clone();
        if let Some(products) = next.get_mut(FIELD).and_then(Value::as_object_mut) {
            products.remove(product);
        }
        Ok(next)
    }).await
}

pub(super) async fn show(product: &str) -> Result<Value, CmdError> {
    let (document, generation) = registry::fetch_versioned_document().await?;
    let targets = destinations::read(&document, product).map_err(CmdError::click)?;
    Ok(json!({"product": product, "state": "declared", "registry_generation": generation, "destinations": targets}))
}
