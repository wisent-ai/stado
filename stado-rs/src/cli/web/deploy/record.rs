//! The registry half a successful deploy leaves behind: the directory entry
//! and the managed record, written together.

use serde_json::{json, Map, Value};

use crate::cli::CmdError;
use crate::config::WebApiProduct;
use crate::declaration::ServiceDeclaration;
use crate::deploy::service::{self, ManagedService};

use super::click;

/// The directory entry and the managed record this deploy leaves behind, in
/// one validated conditional write.
///
/// Both halves in one update, for the reason `service declare` states: the
/// directory contract binds a fixed route to the managed unit on its active
/// host, and a directory entry pointing at no managed service is refused by
/// the validator. The publication counter advances with the entry, because a
/// consumer holding a cached copy otherwise reads a generation telling it the
/// copy is current.
///
/// The transform is pure — the declaration and the record are already decided
/// by the time it runs — which is exactly the shape
/// [`crate::cli::registry::commit_document`] is for: a lost race re-applies
/// the same entry to the newer document.
pub(super) async fn record_declaration(
    product: &str,
    declared: &WebApiProduct,
    record: &ManagedService,
    declaration: &ServiceDeclaration,
) -> Result<String, CmdError> {
    let problems = crate::declaration::validate(
        &format!("service_directory.services.{product}"),
        declaration,
    );
    if !problems.is_empty() {
        return Err(CmdError::click(problems.join("; ")));
    }
    let declaration_value = serde_json::to_value(declaration)?;
    let host = declared.host().to_string();
    let port = declared.port();
    let consumer = declared.consumer().to_string();
    let readyz = declared.readyz().to_string();
    let product = product.to_string();
    let record = record.clone();
    crate::cli::registry::commit_document(move |current| {
        let mut document = current.clone();
        let directory = document
            .get_mut("service_directory")
            .and_then(Value::as_object_mut)
            .ok_or_else(|| {
                CmdError::click(
                    "registry has no service_directory; an authority must publish it before a \
                     web product can be declared",
                )
            })?;
        let services = directory
            .entry("services")
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| CmdError::click("service_directory.services: must be an object"))?;
        let entry = services
            .entry(product.clone())
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .ok_or_else(|| {
                CmdError::click(format!(
                    "service_directory.services.{product}: must be an object"
                ))
            })?;
        entry.insert("active_host".to_string(), json!(&host));
        // The endpoint map is keyed by the active host's registry name, which
        // is a value rather than a literal, so it is built as a map instead of
        // through a JSON literal.
        let mut endpoints = Map::new();
        endpoints.insert(
            host.clone(),
            json!({"url": format!("http://127.0.0.1:{port}")}),
        );
        entry.insert("endpoints".to_string(), Value::Object(endpoints));
        entry.insert("managed_service".to_string(), json!(&product));
        // The unit's own consumer is the caller the directory publishes: a web
        // product reads its own environment and nothing else reads it through
        // this entry. An entry with no consumers answers "who may call this"
        // with silence, which the directory contract refuses.
        let mut consumers = Map::new();
        consumers.insert(consumer.clone(), json!({"capabilities": []}));
        entry.insert("consumers".to_string(), Value::Object(consumers));
        entry.insert(
            "verify".to_string(),
            json!({"kind": "http", "path": &readyz, "expect_status":
                200}),
        );
        entry.insert("declaration".to_string(), declaration_value.clone());

        // Converge rather than insist: a redeploy replaces the record a
        // previous pass wrote, and a first deploy adds it. `add_service`
        // refuses a name it already manages and `replace_service` refuses one
        // it does not, so trying the replacement first is what makes this
        // command re-runnable.
        if service::replace_service(&mut document, &record).is_err() {
            service::add_service(&mut document, &record).map_err(click)?;
        }
        crate::service_resolution::advance_generation(&mut document).map_err(CmdError::click)?;
        Ok(document)
    })
    .await
}
