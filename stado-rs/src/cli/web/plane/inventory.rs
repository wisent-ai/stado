//! `stado web list` and `stado web remove` — reading and withdrawing
//! declarations.

use serde_json::{json, Map, Value};

use super::{mutate_web, product};
use crate::cli::web::{deploy, route, schedules, unit_label};
use crate::cli::CmdError;

pub(crate) fn list(json_output: bool) -> Result<(), CmdError> {
    // An empty plane is not a broken one: nothing declared, in the override
    // or the config file, lists nothing, and a declared table that does not
    // parse is refused with its problems.
    let declared = crate::config::declared_web_api_products().map_err(|problems| {
        CmdError::click(problems.join("; "))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let Some(products) = declared else {
        if json_output {
            println!("[]");
        } else {
            println!("no web products are declared");
        }
        return Ok(());
    };
    let rows: Vec<Map<String, Value>> = products
        .iter()
        .map(|(name, product)| {
            // A hostname-only product has no host, port, consumer or unit,
            // and printing empties for them described a unit that does not
            // exist. Each kind carries the fields it actually has.
            let mut object = Map::new();
            object.insert("product".into(), json!(name));
            object.insert("hostname".into(), json!(product.hostname()));
            object.insert("edge".into(), json!(product.edge()));
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
            object
        })
        .collect();
    let rows: Vec<Value> = with_twins(rows).into_iter().map(Value::Object).collect();
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            print_row(row);
        }
        for row in rows
            .iter()
            .filter(|row| row["twin_declared"] == json!(false))
        {
            let named = |field: &str| match row[field].as_str() {
                Some(name) => name.to_string(),
                None => format!("(no {field} in the row)"),
            };
            println!(
                "missing twin: {} has no declaration for {}",
                named("hostname"),
                named("twin")
            );
        }
    }
    Ok(())
}

/// Every product hostname has an equivalent in both product zones.
const TWIN_ZONES: &[&str] = &["wisent.com", "wisent.ai"];

/// The same name in the other zone, for a hostname in one of the two.
fn twin(hostname: &str) -> Option<String> {
    let &[first, second] = TWIN_ZONES else {
        return None;
    };
    [(first, second), (second, first)]
        .into_iter()
        .find_map(|(zone, other)| {
            if hostname == zone {
                return Some(other.to_string());
            }
            hostname
                .strip_suffix(zone)
                .filter(|label| label.ends_with('.'))
                .map(|label| format!("{label}{other}"))
        })
}

/// Every row whose hostname is in a twin zone, with its twin and whether any
/// declaration owns that twin.
fn with_twins(mut rows: Vec<Map<String, Value>>) -> Vec<Map<String, Value>> {
    let declared: std::collections::BTreeSet<String> = rows
        .iter()
        .filter_map(|row| row.get("hostname").and_then(Value::as_str).map(str::to_string))
        .collect();
    for row in &mut rows {
        let Some(twin) = row.get("hostname").and_then(Value::as_str).and_then(twin) else {
            continue;
        };
        row.insert("twin_declared".into(), json!(declared.contains(&twin)));
        row.insert("twin".into(), json!(twin));
    }
    rows
}

fn print_row(row: &Value) {
    // A field the row does not carry says so, as the port does; it was
    // printed as nothing after its `=`.
    let text = |key: &str| match row[key].as_str() {
        Some(value) => value.to_string(),
        None => "not declared".to_string(),
    };
    let product = text("product");
    let hostname = text("hostname");
    let edge = text("edge");
    // A unit row without a port says so, never as a port of zero.
    let port = match row["port"].as_u64() {
        Some(port) => port.to_string(),
        None => "not declared".to_string(),
    };
    match row["kind"].as_str() {
        Some("redirect") => println!(
            "{product} redirect hostname={hostname} to={} edge={edge}",
            text("redirect_to")
        ),
        Some("upstream-service") => println!(
            "{product} upstream-service hostname={hostname} service={} edge={edge}",
            text("upstream_service")
        ),
        _ => println!(
            "{product} unit host={} port={} hostname={hostname} consumer={} edge={edge} unit={}",
            text("host"),
            port,
            text("consumer"),
            text("unit"),
        ),
    }
}

pub(crate) async fn remove(name: &str, json_output: bool) -> Result<(), CmdError> {
    let declared = product(name)?.clone();
    // Order matters and it is the reverse of publication: the record goes
    // first, so nothing resolves to a unit that is about to stop.
    let record = route::retract(&declared).await?;
    // Nothing is left to answer a scheduled request once the unit retires.
    let schedules = schedules::withdraw(name).await?;
    let unit = deploy::retire(name, &declared).await?;
    let product_name = name.to_string();
    mutate_web("products", |products| {
        products.remove(&product_name);
        Ok(())
    })?;
    let report = json!({
        "product": name,
        "record": record,
        "schedules": schedules,
        "unit": unit,
        "declaration": "removed",
    });
    if json_output {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        // Each receipt is said as it reads: its change, else its state (a
        // hostname-only product's retire answers `no-unit`), else the receipt.
        let said = |receipt: &Value| match receipt["change"].as_str().or_else(|| receipt["state"].as_str()) {
            Some(word) => word.to_string(),
            None => receipt.to_string(),
        };
        println!("{name}: record {}, unit {}, declaration removed", said(&record), said(&unit));
    }
    Ok(())
}
