//! Which products this invocation reports on, and how their verdicts reach
//! the terminal. The one beacon read every row shares is taken here, once,
//! and the non-zero exit is this command's own report of itself rather than a
//! second reading of it.

use std::collections::BTreeMap;

use serde_json::Value;

use super::examine::examine;
use super::VERDICT_SERVING;
use crate::cli::CmdError;
use crate::config::WebApiProduct;
use crate::deploy::{production_runner, service};

/// The products this invocation reports on: one by name, or every declared
/// one.
///
/// An empty plane is not a broken one, exactly as `stado web list` treats it:
/// the plane's parser refuses an empty map so a half-written section cannot
/// pass, so "nothing declared" has to be recognised by the key being absent
/// rather than by the parse failing.
fn selected(
    name: Option<&str>,
) -> Result<Option<BTreeMap<String, &'static WebApiProduct>>, CmdError> {
    if let Some(name) = name {
        let declared = super::product(name)?;
        return Ok(Some(BTreeMap::from([(name.to_string(), declared)])));
    }
    match crate::config::web_api_products() {
        Ok(products) => Ok(Some(
            products
                .iter()
                .map(|(name, product)| (name.clone(), product))
                .collect(),
        )),
        Err(_) if crate::config_file::get("web_api.products").is_none() => Ok(None),
        Err(problems) => Err(CmdError::click(problems.join("; "))),
    }
}

pub(crate) async fn status(name: Option<&str>, json: bool) -> Result<(), CmdError> {
    let Some(products) = selected(name)? else {
        if json {
            println!("[]");
        } else {
            println!("no web products are declared");
        }
        return Ok(());
    };

    // One beacon read for every product, not one per product: the join is
    // fleet-wide already and asking again per row would pay a store listing
    // for each.
    let store = crate::cli::host::beacon_store().await?;
    let managed = service::list_services(&store).await.map_err(|error| {
        CmdError::click(format!(
            "the managed service set could not be read, so no unit state below could be judged: \
             {error}"
        ))
    })?;
    let runner = production_runner();

    let mut rows: Vec<Value> = Vec::with_capacity(products.len());
    let mut broken: Vec<String> = Vec::new();
    for (product, declared) in &products {
        let unit = super::unit_label(product);
        // Both spellings resolve, because both are how this unit is addressed:
        // the product's own name is what the registry records it under and the
        // label is what the host calls it.
        let row = managed
            .iter()
            .find(|row| row.service.matches(product) || row.service.matches(&unit));
        let verdict = examine(product, declared, row, &managed, &runner).await;
        if verdict.word != VERDICT_SERVING {
            broken.push(format!("{product}: {}", verdict.word));
        }
        rows.push(verdict.row);
    }

    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            println!(
                "{} {} host={} port={} unit={} hostname={}",
                row["product"].as_str().unwrap_or_default(),
                row["verdict"].as_str().unwrap_or_default(),
                row["host"].as_str().unwrap_or_default(),
                row["port"].as_u64().unwrap_or_default(),
                row["unit"].as_str().unwrap_or_default(),
                row["hostname"].as_str().unwrap_or_default(),
            );
            println!(
                "  unit: {} (reported {})",
                row["unit_state"].as_str().unwrap_or("unknown"),
                match row["unit_reported_at"].as_str() {
                    Some(stamp) if !stamp.is_empty() => stamp,
                    _ => "never",
                }
            );
            println!(
                "  port: {}{}",
                row["port_state"].as_str().unwrap_or("unasked"),
                match row["port_detail"].as_str() {
                    Some(detail) if !detail.is_empty() => format!(" — {detail}"),
                    _ => String::new(),
                }
            );
            println!(
                "  dns:  {} — {}",
                row["dns_state"].as_str().unwrap_or("unknown"),
                row["dns_detail"].as_str().unwrap_or_default(),
            );
            if let Some(problem) = row["edge_error"].as_str() {
                println!("  edge: {problem}");
            }
        }
    }

    if !broken.is_empty() {
        // Said on stderr so the table above stays parseable, and the command
        // exits non-zero on its own report rather than on a second reading of
        // it.
        eprintln!("not serving: {}", broken.join("; "));
        return Err(CmdError::silent(1));
    }
    Ok(())
}
