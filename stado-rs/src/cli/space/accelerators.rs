//! The accelerators block of `stado space report`: who holds the host's
//! cards, read from its latest capacity publication.

use serde_json::{json, Value};

/// Who holds the host's accelerators, read from its latest capacity
/// publication rather than from a second driver query, so this report and
/// the agent never disagree. A host with no publication, or a store that
/// cannot be read, says so in `error` instead of showing an empty card.
pub(super) async fn accelerators_json(target: &crate::targets::ComputeTarget) -> Value {
    let registry = match crate::cli::registry::read_registry().await {
        Ok(registry) => registry,
        Err(error) => return json!({"error": error.to_string()}),
    };
    let store = match crate::queue::submit::default_store("").await {
        Ok(store) => store,
        Err(error) => return json!({"error": error.to_string()}),
    };
    let publications = match crate::queue::capacity::read_publications(&store).await {
        Ok(publications) => publications,
        Err(error) => return json!({"error": error.to_string()}),
    };
    let Some((consumer, publication)) = publications.iter().find(|(consumer, _)| {
        crate::queue::capacity::consumer_names_target(&registry, target, consumer)
    }) else {
        return json!({"error": format!("no capacity publication names {}", target.name)});
    };
    let now = chrono::Utc::now();
    let payload = &publication.payload;
    let diag = payload
        .get("diag")
        .and_then(Value::as_object)
        .cloned()
        .unwrap_or_default();
    let line = crate::providers::local::accelerators::accelerator_holders_line(&diag).map(|line| {
        if publication.stale(now) {
            format!("{line} (publication stale)")
        } else {
            line
        }
    });
    json!({
        "consumer_id": consumer,
        "published_at": payload.get("published_at"),
        "stale": publication.stale(now),
        "total_vram_gb": payload.get("total_vram_gb"),
        "free_vram_gb": payload.get("free_vram_gb"),
        "memory_model": diag.get("accelerator_memory_model"),
        "holders": diag.get("accelerator_holders").cloned().unwrap_or_else(|| json!([])),
        "unattributed_gb": diag.get("vram_unattributed_gb"),
        "holders_error": diag.get("accelerator_holders_error"),
        "line": line,
    })
}
