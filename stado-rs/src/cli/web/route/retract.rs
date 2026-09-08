//! Unpublishing: the record goes first, then the hostname — or the one
//! `handle_path` block a mount owns — comes out of the edge.

use serde_json::{json, Value};

use super::cloudflare::cloudflare_unavailable;
use super::{CmdError, RECORD_TYPE, REGISTRAR_CREDENTIAL};
use crate::config::WebApiProduct;

/// Drop the hostname's record and stop the edge terminating it.
///
/// The record first, deliberately: after it is gone nothing resolves to the
/// edge for this name, so removing the site block cannot strand a live
/// request. `stado web remove` calls this while the product is still declared
/// — the declaration is only forgotten once the unit is retired — which is why
/// the desired route set handed to the edge is computed here by excluding this
/// hostname rather than by re-reading the declarations.
pub(crate) async fn retract(name: &str, declared: &WebApiProduct) -> Result<Value, CmdError> {
    match declared.edge() {
        "cloudflare" => Ok(json!({
            "hostname": declared.hostname(),
            "change": "unchanged",
            "refused": cloudflare_unavailable(declared.hostname()),
        })),
        "stado" => {
            // A mount does not own the hostname, so retracting it must not
            // touch the record: the owner still answers there and every other
            // mount under it still does too. What comes out is one
            // `handle_path` block, and nothing else.
            if let Some(prefix) = declared.path_prefix() {
                let edge = match crate::config::web_api_edge() {
                    Ok(edge) => edge,
                    Err(_) => {
                        return Ok(json!({
                            "hostname": declared.hostname(),
                            "path_prefix": prefix,
                            "change": "unchanged",
                            "record": Value::Null,
                            "edge": Value::Null,
                        }))
                    }
                };
                let mounted = super::super::edge::mount(
                    declared.hostname(),
                    prefix,
                    declared.host(),
                    declared.port(),
                )?;
                let routes: Vec<(String, Vec<String>)> = super::super::edge::stado_routes()
                    .await?
                    .into_iter()
                    .map(|(hostname, mut block)| {
                        if hostname == declared.hostname() {
                            block.retain(|directive| directive != &mounted.1);
                        }
                        (hostname, block)
                    })
                    .collect();
                let edge_report = super::super::edge::deliver(edge, &routes, true).await?;
                return Ok(json!({
                    "hostname": declared.hostname(),
                    "path_prefix": prefix,
                    "change": "removed",
                    // The record belongs to whichever declaration owns this
                    // hostname, and it is still published there.
                    "record": Value::Null,
                    "edge": edge_report,
                }));
            }
            let record = crate::cli::dns::remove_record(
                declared.hostname(),
                RECORD_TYPE,
                None,
                REGISTRAR_CREDENTIAL,
            )
            .await?;
            let removed = record["removed"].as_u64().unwrap_or_default() > 0;
            // An undeclared edge is not a failed retraction: the record is
            // already gone, so the hostname is unpublished, and there is no
            // proxy configuration for it to still appear in.
            let edge = match crate::config::web_api_edge() {
                Ok(edge) => edge,
                Err(_) => {
                    return Ok(json!({
                        "hostname": declared.hostname(),
                        "change": if removed { "removed" } else { "unchanged" },
                        "record": record,
                        "edge": Value::Null,
                    }))
                }
            };
            let routes: Vec<(String, Vec<String>)> = super::super::edge::stado_routes()
                .await?
                .into_iter()
                .filter(|(hostname, _)| hostname != declared.hostname())
                .collect();
            let edge_report = super::super::edge::deliver(edge, &routes, true).await?;
            Ok(json!({
                "hostname": declared.hostname(),
                "change": if removed { "removed" } else { "unchanged" },
                "record": record,
                "edge": edge_report,
            }))
        }
        other => Err(CmdError::click(format!(
            "web product {name} declares edge {other:?}, and no retraction path implements it"
        ))),
    }
}
