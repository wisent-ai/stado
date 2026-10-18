//! Publishing: the edge, then the record, then the proof — and the URL that
//! proof is read from.
//!
//! The order is the whole design, and it lives in one function so it cannot
//! be reordered by accident. `verification.rs` holds the third step, which is
//! the only one that waits.

use serde_json::json;

use super::planning::{planned_record, resolved_words};
use super::{registrar_credential, CmdError, RECORD_TYPE};
use crate::cli::reporting::field;
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
    // A hostname whose record this command writes needs the registrar item
    // before anything changes, so the edge is never left terminating a name
    // whose record could not then be written.
    let registrar = if check || declared.path_prefix().is_some() {
        None
    } else {
        Some(registrar_credential(edge)?)
    };
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
                field(&edge_report["change"]),
                field(&record["change"]),
                resolved_words(&record),
                edge.address(),
            );
        }
        // Nothing was written, so the only useful answer to "is this already
        // published" is the exit code.
        return if settled {
            Ok(())
        } else {
            Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE))
        };
    }

    // A mount writes no record, and only a mount reaches here without the
    // registrar item. The hostname's A record belongs to the declaration that
    // owns it and already points at this edge; writing it again from here
    // would be a second writer of one name, and removing this mount would then
    // look like it should take the record with it.
    let record = match registrar {
        None => json!({
            "change": "unchanged",
            "detail": format!(
                "{} is owned by another declaration, whose record already points at this edge",
                declared.hostname()
            ),
        }),
        Some(registrar) => {
            crate::cli::dns::ensure_record(
                declared.hostname(),
                RECORD_TYPE,
                edge.address(),
                None,
                None,
                registrar,
            )
            .await?
        }
    };
    let served = verify(declared).await?;
    // Only now, with the hostname answering from this fleet, do the
    // product's scheduled requests start here: before the cutover they are
    // still sent by whatever served the product, and sending them from both
    // would run every one twice.
    let schedules = crate::cli::web::schedules::activate(name, declared).await?;
    let mut report = json!({
        "product": name,
        "hostname": declared.hostname(),
        "edge": edge_report,
        "record": record,
        "served": served,
        "schedules": schedules,
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
            field(&served["status"]),
            field(&served["server"]),
            field(&report["edge"]["change"]),
            field(&report["record"]["change"]),
        );
        for row in report["schedules"].as_array().into_iter().flatten() {
            println!("  schedule {} {}", field(&row["id"]), field(&row["change"]));
        }
    }
    Ok(())
}
