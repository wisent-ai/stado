//! The declared memory policies as an operator client sees them.
//!
//! Two shapes, both read-only. `GET /api/memory-policies.json` is the catalog
//! this build carries, so a console can offer the same named policies `stado
//! space policies` lists and post the one an operator picks through the
//! existing `POST /api/registry/policy`. The per-target block travels inside
//! `GET /api/registry.json`, because the fit and the verdict are facts about
//! one host.
//!
//! The verdict is not computed here. It is the same function the CLI prints
//! from, for the reason the whole capability exists: a surface that showed
//! the fields and left the judgement to whoever was reading is what let a
//! `report`-mode declaration look like management on charless-mac-mini while
//! its pre-check runner was being killed for memory.

use serde_json::{json, Value};

use super::{http_status, send_json, Response};
use crate::providers::local::host_memory::declaration::policies;

/// `GET /api/memory-policies.json`
pub(crate) fn get_memory_policies() -> Response {
    match policies::all() {
        Ok(declared) => send_json(
            http_status("200"),
            &json!({
                "declaration": policies::DECLARATION_PATH,
                "policies": declared
                    .iter()
                    .map(policies::policy_json)
                    .collect::<Vec<Value>>(),
            }),
        ),
        Err(error) => send_json(
            http_status("500"),
            &json!({"error": format!("the declared memory policies are unreadable: {error}")}),
        ),
    }
}

/// Which declared policies this target may be armed with, and whether what it
/// carries repairs anything.
pub(super) fn projected_for(entry: &serde_json::Map<String, Value>) -> Value {
    let platform = entry
        .get("release_platform")
        .and_then(Value::as_str)
        .unwrap_or_default();
    let role = entry.get("role").and_then(Value::as_str);
    json!({
        "automatic": policies::automatic_verdict(entry.get("memory_reclaim")),
        "fitting": policies::fitting(platform, role)
            .iter()
            .map(|policy| policy.name.as_str())
            .collect::<Vec<&str>>(),
    })
}
