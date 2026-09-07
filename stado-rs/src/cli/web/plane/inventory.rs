//! `stado web list` and `stado web remove` — reading and withdrawing
//! declarations.

use serde_json::{json, Value};

use super::{mutate_web, product};
use crate::cli::web::{deploy, route, unit_label};
use crate::cli::CmdError;

pub(crate) fn list(json_output: bool) -> Result<(), CmdError> {
    let products = match crate::config::web_api_products() {
        Ok(products) => products,
        // An empty plane is not a broken one: the parser refuses an empty map
        // so that a half-written section cannot pass, and "nothing declared"
        // has to read as nothing declared.
        Err(_) if crate::config_file::get("web_api.products").is_none() => {
            if json_output {
                println!("[]");
            } else {
                println!("no web products are declared");
            }
            return Ok(());
        }
        Err(problems) => return Err(CmdError::click(problems.join("; "))),
    };
    let rows: Vec<Value> = products
        .iter()
        .map(|(name, product)| {
            // A hostname-only product has no host, port, consumer or unit,
            // and printing empties for them described a unit that does not
            // exist. Each kind carries the fields it actually has.
            let mut row = json!({
                "product": name,
                "hostname": product.hostname(),
                "edge": product.edge(),
            });
            let object = row.as_object_mut().expect("a JSON object was just built");
            match (product.redirect_to(), product.upstream_service()) {
                (Some(target), _) => {
                    object.insert("kind".into(), json!("redirect"));
                    object.insert("redirect_to".into(), json!(target));
                }
                (None, Some(service)) => {
                    object.insert("kind".into(), json!("upstream-service"));
                    object.insert("upstream_service".into(), json!(service));
                }
                (None, None) => {
                    object.insert("kind".into(), json!("unit"));
                    object.insert("host".into(), json!(product.host()));
                    object.insert("port".into(), json!(product.port()));
                    object.insert("consumer".into(), json!(product.consumer()));
                    object.insert("unit".into(), json!(unit_label(name)));
                    object.insert("readyz".into(), json!(product.readyz()));
                    object.insert(
                        "database".into(),
                        json!(product.database().map(|database| json!({
                            "name": database.name(),
                            "field": database.field(),
                            "variable": database.variable(),
                        }))),
                    );
                    object.insert(
                        "secrets".into(),
                        json!(product.secrets().keys().cloned().collect::<Vec<_>>()),
                    );
                }
            }
            row
        })
        .collect();
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            print_row(row);
        }
    }
    Ok(())
}

fn print_row(row: &Value) {
    let text = |key: &str| row[key].as_str().unwrap_or("");
    let product = text("product");
    let hostname = text("hostname");
    let edge = text("edge");
    match text("kind") {
        "redirect" => println!(
            "{product} redirect hostname={hostname} to={} edge={edge}",
            text("redirect_to")
        ),
        "upstream-service" => println!(
            "{product} upstream-service hostname={hostname} service={} edge={edge}",
            text("upstream_service")
        ),
        _ => println!(
            "{product} unit host={} port={} hostname={hostname} consumer={} edge={edge} unit={}",
            text("host"),
            row["port"].as_u64().unwrap_or(0),
            text("consumer"),
            text("unit"),
        ),
    }
}

pub(crate) async fn remove(name: &str, json_output: bool) -> Result<(), CmdError> {
    let declared = product(name)?.clone();
    // Order matters and it is the reverse of publication: the record goes
    // first, so nothing resolves to a unit that is about to stop.
    let record = route::retract(name, &declared).await?;
    let unit = deploy::retire(name, &declared).await?;
    let product_name = name.to_string();
    mutate_web("products", |products| {
        products.remove(&product_name);
        Ok(())
    })?;
    let report = json!({
        "product": name,
        "record": record,
        "unit": unit,
        "declaration": "removed",
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        println!(
            "{name}: record {}, unit {}, declaration removed",
            record["change"].as_str().unwrap_or("unchanged"),
            unit["change"].as_str().unwrap_or("unchanged"),
        );
    }
    Ok(())
}
