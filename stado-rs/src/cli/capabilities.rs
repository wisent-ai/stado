//! `stado capabilities` — discover user-facing features and provider support,
//! and, for what one product calls another for, the selected order of its
//! alternatives, each alternative's last proof and which one served which
//! consumer (`resolve`, `select`).

mod record;
mod resolve;

use serde::Serialize;
use serde_json::{json, Value};

use super::CmdError;
use crate::capabilities::{self, alternatives, ProductCapability};

pub(crate) use resolve::run as resolve;

#[derive(Serialize)]
struct CatalogDocument<'a> {
    schema: &'static str,
    capabilities: Vec<&'a ProductCapability>,
    /// Each family a product resolves through Stado: its selected order,
    /// each alternative's last proof, and what served each consumer.
    alternatives: Vec<Value>,
    control_config: &'static [crate::capabilities::ConfigField],
}

/// The selected order, proofs and served consumers of every family among
/// `selected`, as `stado capabilities` prints them.
fn families(selected: &[&ProductCapability]) -> Result<Vec<Value>, CmdError> {
    let kept = record::read()?;
    let mut rows = Vec::new();
    for family in alternatives::FAMILIES {
        if !selected.iter().any(|capability| capability.id == family.id) {
            continue;
        }
        let order: Vec<&str> = resolve::order(family)?
            .iter()
            .map(|alternative| alternative.provider.as_str())
            .collect();
        let record = kept
            .families
            .get(family.id.as_str())
            .cloned()
            .unwrap_or_default();
        rows.push(json!({
            "family": family.id.as_str(),
            "order": order,
            "order_key": family.order.path,
            "proven": record.proven,
            "served": record.served,
        }));
    }
    Ok(rows)
}

pub fn run(filter: Option<&str>, as_json: bool) -> Result<(), CmdError> {
    let selected = match filter {
        Some(id) => vec![capabilities::product_capability(id).ok_or_else(|| {
            let choices = capabilities::product_capabilities()
                .iter()
                .map(|capability| capability.id.as_str())
                .collect::<Vec<_>>()
                .join(", ");
            CmdError::usage(format!("unknown capability {id:?}; use one of: {choices}"))
        })?],
        None => capabilities::product_capabilities().iter().collect(),
    };
    let families = families(&selected)?;

    if as_json {
        let document = CatalogDocument {
            schema: "product-v1",
            capabilities: selected,
            alternatives: families,
            control_config: capabilities::CONTROL_CONFIG,
        };
        println!("{}", serde_json::to_string_pretty(&document)?);
        return Ok(());
    }

    let rows = selected
        .into_iter()
        .flat_map(|capability| {
            capability.providers.iter().map(move |provider| {
                vec![
                    capability.id.as_str().to_string(),
                    provider.provider.as_str().to_string(),
                    provider.support.as_str().to_string(),
                    if provider.implementation.is_empty() {
                        "(none)".to_string()
                    } else {
                        provider.implementation.to_string()
                    },
                    provider.note.to_string(),
                ]
            })
        })
        .collect::<Vec<_>>();
    crate::cli::reporting::table::print(
        &[
            "CAPABILITY",
            "PROVIDER",
            "SUPPORT",
            "IMPLEMENTATION",
            "NOTE",
        ],
        &rows,
    );
    for family in &families {
        let order: Vec<&str> = family["order"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        println!();
        println!(
            "{}: tried in order {} ({})",
            family["family"].as_str().unwrap_or_default(),
            order.join(", "),
            family["order_key"].as_str().unwrap_or_default()
        );
        for (name, proof) in family["proven"].as_object().into_iter().flatten() {
            let state = if proof["answered"].as_bool() == Some(true) {
                "answered".to_owned()
            } else {
                format!(
                    "did not answer: {}",
                    proof["cause"].as_str().unwrap_or("no cause kept")
                )
            };
            println!(
                "  {name} at {} {state} ({})",
                proof["route"].as_str().unwrap_or_default(),
                proof["at"].as_str().unwrap_or_default()
            );
        }
        for (consumer, served) in family["served"].as_object().into_iter().flatten() {
            let by = match served["provider"].as_str() {
                Some(provider) if served["fallback"].as_bool() == Some(true) => {
                    format!("{provider} (a fallback)")
                }
                Some(provider) => provider.to_owned(),
                None => "nothing: no alternative answered".to_owned(),
            };
            println!(
                "  {consumer} served by {by} ({})",
                served["at"].as_str().unwrap_or_default()
            );
        }
    }
    Ok(())
}

/// `stado capabilities select FAMILY --order A,B`: write the family's
/// selected order to `capabilities.<family>.order` in the config file, after
/// refusing a name the family does not declare or names twice.
pub(crate) fn select(family: &str, order: &[String]) -> Result<(), CmdError> {
    let declared = alternatives::family(family).ok_or_else(|| {
        let known: Vec<&str> = alternatives::FAMILIES
            .iter()
            .map(|family| family.id.as_str())
            .collect();
        CmdError::usage(format!(
            "{family:?} is no family a product resolves through Stado; the families are [{}]",
            known.join(", ")
        ))
    })?;
    if order.is_empty() {
        return Err(CmdError::usage(format!(
            "--order names {}'s alternatives in the order they are tried: [{}]",
            declared.id,
            declared.names().join(", ")
        )));
    }
    let names: Vec<&str> = declared
        .ordered(order)
        .map_err(CmdError::usage)?
        .iter()
        .map(|alternative| alternative.provider.as_str())
        .collect();
    crate::cli::config_cmd::keys::set(declared.order.path, &serde_json::to_string(&names)?)
}
