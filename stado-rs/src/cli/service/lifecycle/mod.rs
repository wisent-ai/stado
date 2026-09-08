//! The write side of `stado service`: declaring a unit, asserting it,
//! adopting one the registry does not know about, releasing a new version of
//! one, and forgetting one.
//!
//! [`record_declaration`], [`render_mutation`] and [`now`] are here because
//! every family below writes the same document through the same validated
//! conditional path and reports the result in the same shape.

use super::*;

pub(crate) mod adopt;
pub(crate) mod declare;
pub(crate) mod deploy;
pub(crate) mod release;

// ---------------------------------------------------------------------------
// Adopt / retire / deploy — the registry mutations
// ---------------------------------------------------------------------------

/// Declare a service through the validated conditional write path.
///
/// `commit_document` runs `targets::validate_registry` before it writes, so a
/// declaration that would produce an invalid registry is refused with nothing
/// uploaded. Pure: the record is already decided, and adding it is a function
/// of the document it is added to, so a lost race re-applies it to the newer
/// document. Returns the new generation.
async fn record_declaration(record: &ManagedService) -> Result<String, CmdError> {
    registry::commit_document(|current| {
        let mut document = current.clone();
        // A placeholder left by `service declare` is the declaration, not the
        // unit: deploy replaces it with the real record rather than refusing on
        // a name it put there itself.
        if let Some(services) = document
            .get_mut("targets")
            .and_then(Value::as_array_mut)
            .and_then(|targets| {
                targets
                    .iter_mut()
                    .find(|target| {
                        target.get("name").and_then(Value::as_str) == Some(record.host.as_str())
                    })
                    .and_then(|target| target.get_mut("services"))
                    .and_then(Value::as_array_mut)
            })
        {
            services.retain(|existing| {
                !(existing.get("name").and_then(Value::as_str) == Some(record.name.as_str())
                    && existing.get("declared_only").and_then(Value::as_bool) == Some(true))
            });
        }
        service::add_service(&mut document, record).map_err(click)?;
        Ok(document)
    })
    .await
}

/// `datetime.now(timezone.utc).isoformat()` as every other writer in the
/// crate stamps it (`queue/leases.rs::now_iso`).
fn now() -> String {
    chrono::Utc::now().to_rfc3339()
}

fn render_mutation(
    action: &str,
    record: &ManagedService,
    generation: &str,
    remote: Option<&Value>,
    json: bool,
) -> Result<(), CmdError> {
    if json {
        let mut payload = serde_json::json!({
            "action": action,
            "service": record.to_json(),
            "registry_generation": generation,
        });
        if let Some(remote) = remote {
            payload["remote"] = remote.clone();
        }
        return print_json(&payload);
    }
    table::print(
        &[
            "ACTION",
            "HOST",
            "SERVICE",
            "UNIT",
            "KIND",
            "PATH",
            "GENERATION",
        ],
        &[vec![
            action.to_string(),
            record.host.clone(),
            record.name.clone(),
            record.unit_id().to_string(),
            record.kind.clone(),
            dash(&record.path),
            generation.to_string(),
        ]],
    );
    Ok(())
}
