//! `stado capabilities` — discover user-facing features and provider support,
//! and, for what one product calls another for, the selected order of its
//! alternatives, each alternative's last proof and which one served which
//! consumer (`resolve`, `select`); the request itself sent to the
//! alternative that answers (`call`), and the statements that answer a
//! request in place of a product (`attestation`).

mod attestation;
mod call;
mod record;
mod resolve;

use serde::Serialize;
use serde_json::{json, Value};

use super::CmdError;
use crate::capabilities::{self, alternatives, ProductCapability};

pub(crate) use attestation::{list as attestation_list, record as attestation_record};
pub(crate) use attestation::{revoke as attestation_revoke, show as attestation_show};
pub(crate) use call::run as call;
pub(crate) use resolve::run as resolve;

#[derive(Serialize)]
struct CatalogDocument<'a> {
    schema: &'static str,
    capabilities: Vec<&'a ProductCapability>,
    /// Each family a product resolves through Stado: its selected order,
    /// each alternative's last proof, and what served each consumer.
    alternatives: Vec<Value>,
    /// With the market family selected, each seller's readiness on this
    /// host; empty otherwise, since asking every marketplace is a network
    /// read the bare catalog does not make.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    sellers: Vec<Value>,
    control_config: &'static [crate::capabilities::ConfigField],
}

/// Each seller the market family declares, with whether this host can sell
/// through it now and the seller's own sentence why, as the seller's adapter
/// answers it (each declared peer fleet is its own row), and for a seller
/// Stado has no adapter for, its support level and the catalog's note. A
/// seller that is not ready is named, never left out, and does not stop the
/// others being read.
async fn sellers(market: &ProductCapability) -> Vec<Value> {
    let mut rows = Vec::new();
    // The selected order first, then every seller it does not name, in the
    // catalog's order; an order that cannot be read is said and the
    // catalog's order stands.
    let ordered = match market_order(market) {
        Ok(ordered) => ordered,
        Err(cause) => {
            rows.push(serde_json::json!({
                "provider": "order",
                "ready": false,
                "verdict": cause,
            }));
            market.providers.iter().collect()
        }
    };
    for row in ordered {
        let adapter = crate::cli::MarketProvider::of(row)
            .map(|provider| crate::cli::integrations::market::adapter(provider, None));
        match adapter {
            Some(Ok(adapter)) => rows.extend(adapter.verdicts().await),
            Some(Err(error)) => rows.push(json!({
                "provider": row.provider.as_str(),
                "ready": false,
                "verdict": error.to_string(),
            })),
            None => rows.push(json!({
                "provider": row.provider.as_str(),
                "ready": false,
                "verdict": format!(
                    "{}: Stado carries no adapter ({})",
                    row.support.as_str(),
                    row.note
                ),
            })),
        }
    }
    rows
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

pub async fn run(filter: Option<&str>, as_json: bool) -> Result<(), CmdError> {
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
    let sellers = match selected.as_slice() {
        [only] if only.id == capabilities::CapabilityKind::Market => sellers(only).await,
        _ => Vec::new(),
    };

    if as_json {
        let document = CatalogDocument {
            schema: "product-v1",
            capabilities: selected,
            alternatives: families,
            sellers,
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
    if !sellers.is_empty() {
        println!();
        println!("sellers on this host:");
        for seller in &sellers {
            let name = match seller["peer"].as_str() {
                Some(peer) => format!("{} {peer}", seller["provider"].as_str().unwrap_or("?")),
                None => seller["provider"].as_str().unwrap_or("?").to_string(),
            };
            let state = if seller["ready"].as_bool() == Some(true) {
                "ready"
            } else {
                "not ready"
            };
            println!(
                "  {name}: {state}: {}",
                seller["verdict"].as_str().unwrap_or_default()
            );
        }
    }
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

/// The family `id` names, else a usage refusal naming every family.
fn declared_family(id: &str) -> Result<&'static alternatives::Family, CmdError> {
    alternatives::family(id).ok_or_else(|| {
        let known: Vec<&str> = alternatives::FAMILIES
            .iter()
            .map(|family| family.id.as_str())
            .collect();
        CmdError::usage(format!(
            "{id:?} is no family a product resolves through Stado; the families are [{}]",
            known.join(", ")
        ))
    })
}

/// `stado capabilities select FAMILY --order A,B`: write the family's
/// selected order to `capabilities.<family>.order` in the config file, after
/// refusing a name the family does not declare or names twice.
pub(crate) fn select(family: &str, order: &[String]) -> Result<(), CmdError> {
    if family == capabilities::CapabilityKind::Market.as_str() {
        return select_market(order);
    }
    let declared = declared_family(family)?;
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

/// The market family's sellers in the selected order
/// (`capabilities.market.order`), followed by every seller it does not
/// name, in the catalog's order. A name the family does not declare is the
/// refusal, naming the sellers.
fn market_order(
    market: &ProductCapability,
) -> Result<Vec<&'static crate::capabilities::ProviderCapability>, String> {
    let field = &alternatives::MARKET_ORDER_CONFIG;
    let selected: Vec<String> = match crate::config_file::field_value(field) {
        Some(Value::Array(items)) => items
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(text)) => text.split(',').map(|name| name.trim().to_owned()).collect(),
        Some(other) => return Err(format!("{} is {other}, not a list of sellers", field.path)),
        None => Vec::new(),
    };
    let providers: &'static [crate::capabilities::ProviderCapability] = market.providers;
    let mut ordered = Vec::new();
    for name in &selected {
        let row = providers
            .iter()
            .find(|row| row.provider.matches(name))
            .ok_or_else(|| {
                format!(
                    "{} names {name:?}, which the market family does not declare; it declares {}",
                    field.path,
                    providers
                        .iter()
                        .map(|row| row.provider.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            })?;
        if !ordered.contains(&row) {
            ordered.push(row);
        }
    }
    for row in providers {
        if !ordered.contains(&row) {
            ordered.push(row);
        }
    }
    Ok(ordered)
}

/// `stado capabilities select market --order A,B`: write the sellers' order
/// to `capabilities.market.order`, refusing a seller the family does not
/// declare or names twice.
fn select_market(order: &[String]) -> Result<(), CmdError> {
    let market = capabilities::product_capability(capabilities::CapabilityKind::Market.as_str())
        .ok_or_else(|| CmdError::declaration("the capability catalog declares no market family"))?;
    let declared: Vec<&str> = market
        .providers
        .iter()
        .map(|row| row.provider.as_str())
        .collect();
    if order.is_empty() {
        return Err(CmdError::usage(format!(
            "--order names the market's sellers in the order they are offered: [{}]",
            declared.join(", ")
        )));
    }
    let mut names: Vec<&str> = Vec::new();
    for name in order {
        let row = market
            .providers
            .iter()
            .find(|row| row.provider.matches(name.trim()))
            .ok_or_else(|| {
                CmdError::usage(format!(
                    "the market family has no seller {name:?}; its sellers are [{}]",
                    declared.join(", ")
                ))
            })?;
        if names.contains(&row.provider.as_str()) {
            return Err(CmdError::usage(format!("--order names {name:?} twice")));
        }
        names.push(row.provider.as_str());
    }
    crate::cli::config_cmd::keys::set(
        alternatives::MARKET_ORDER_CONFIG.path,
        &serde_json::to_string(&names)?,
    )
}
