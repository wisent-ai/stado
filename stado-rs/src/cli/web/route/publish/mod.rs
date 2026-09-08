//! Publishing: the edge, then the record, then the proof — and the URL that
//! proof is read from.
//!
//! The order is the whole design, and it lives in one function so it cannot
//! be reordered by accident. `verification.rs` holds the third step, which is
//! the only one that waits.

use serde_json::json;

use super::planning::{planned_record, resolved_words};
use super::{CmdError, RECORD_TTL, RECORD_TYPE, REGISTRAR_CREDENTIAL};
use crate::config::WebApiProduct;

mod verification;

use verification::verify;

/// The URL a product's publication is proved by.
///
/// A mount is proved at its own prefix: the owner's `readyz` is a path on the
/// owner's application and says nothing about whether `/docs` reaches this
/// unit. The trailing slash is deliberate — `/docs` and `/docs/` both match
/// the mount's matcher, and the slash is what the mount's own root resolves
/// to.
fn verify_url(declared: &WebApiProduct) -> String {
    match declared.path_prefix() {
        Some(prefix) => format!("https://{}{prefix}/", declared.hostname()),
        None => format!("https://{}{}", declared.hostname(), declared.readyz()),
    }
}

pub(super) async fn publish(
    name: &str,
    declared: &WebApiProduct,
    check: bool,
    json_output: bool,
) -> Result<(), CmdError> {
    let edge = super::super::edge::declared()?;
    let routes = super::super::edge::stado_routes().await?;
    // The edge first, always. `check` false is what makes this the write; with
    // `check` true nothing is delivered and nothing is written locally either.
    let edge_report = super::super::edge::deliver(edge, &routes, !check).await?;

    if check {
        let record = planned_record(declared, edge).await?;
        let settled = edge_report["change"].as_str() == Some("unchanged")
            && record["change"].as_str() == Some("unchanged");
        let report = json!({
            "product": name,
            "hostname": declared.hostname(),
            "edge": edge_report,
            "record": record,
            "change": if settled { "unchanged" } else { "would-change" },
        });
        if json_output {
            println!("{}", serde_json::to_string_pretty(&report)?);
        } else {
            println!(
                "{}: edge {}, record {} ({} -> {})",
                declared.hostname(),
                edge_report["change"].as_str().unwrap_or_default(),
                record["change"].as_str().unwrap_or_default(),
                resolved_words(&record),
                edge.address(),
            );
        }
        // Nothing was written, so the only useful answer to "is this already
        // published" is the exit code.
        return if settled {
            Ok(())
        } else {
            Err(CmdError::silent(1))
        };
    }

    // A mount writes no record. The hostname's A record belongs to the
    // declaration that owns it and already points at this edge; writing it
    // again from here would be a second writer of one name, and removing this
    // mount would then look like it should take the record with it.
    let record = if declared.path_prefix().is_some() {
        json!({
            "change": "unchanged",
            "detail": format!(
                "{} is owned by another declaration, whose record already points at this edge",
                declared.hostname()
            ),
        })
    } else {
        crate::cli::dns::ensure_record(
            declared.hostname(),
            RECORD_TYPE,
            edge.address(),
            RECORD_TTL,
            None,
            REGISTRAR_CREDENTIAL,
        )
        .await?
    };
    let served = verify(declared).await?;
    let mut report = json!({
        "product": name,
        "hostname": declared.hostname(),
        "edge": edge_report,
        "record": record,
        "served": served,
        "change": "published",
    });
    if let Some(prefix) = declared.path_prefix() {
        report["path_prefix"] = json!(prefix);
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{} answers {} from {} (edge {}, record {})",
            verify_url(declared),
            served["status"].as_u64().unwrap_or_default(),
            served["server"].as_str().unwrap_or("an unnamed server"),
            report["edge"]["change"].as_str().unwrap_or_default(),
            report["record"]["change"].as_str().unwrap_or_default(),
        );
    }
    Ok(())
}
