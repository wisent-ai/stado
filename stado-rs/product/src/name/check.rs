//! `stado product name check NAME` and `stado product name forget PRODUCT
//! FORMER`: whether a name is free, and removing a former name recorded in
//! error.

use super::{field, former_rows, holder, key, position, products, read_for_write};
use crate::{
    catalog::{self, text},
    common::{emit, Arguments, Runtime},
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeMap;

/// The catalog product that claims each repository, `owner/name`.
fn claims(document: &Value) -> Result<BTreeMap<String, String>> {
    let mut claims = BTreeMap::new();
    for product in products(document)? {
        let id = text(product, "id")?;
        for repository in crate::dependencies::repositories(product) {
            claims.insert(repository, id.to_owned());
        }
    }
    Ok(claims)
}

pub(super) fn check(runtime: &Runtime, args: &Arguments, json: bool) -> Result<i32> {
    let [name] = args.positional.as_slice() else {
        bail!("name check takes one NAME");
    };
    if args.has("--as-id") {
        crate::common::slug(name).with_context(|| format!("{name:?} cannot be a product id"))?;
    }
    let document = catalog::current(runtime)?;
    let mut taken = Vec::new();
    if let Some(held) = holder(&document, name)? {
        taken.push(json!({"held_by": held.id, "as": held.held_as}));
    }
    // A repository named for the word, or for one of its surfaces
    // (`<name>-desktop`), is the name in use too.
    if let Some(org) = args.optional("--org")? {
        let claims = claims(&document)?;
        let wanted = key(name);
        for repository in catalog::organization_repositories(org)? {
            let Some(full) = repository["nameWithOwner"].as_str() else {
                bail!("`gh repo list {org}` gave a repository no nameWithOwner: {repository}");
            };
            let Some((_, short)) = full.split_once('/') else {
                bail!("`gh repo list {org}` gave {full}, which is not OWNER/NAME");
            };
            let short = key(short);
            if short == wanted || short.starts_with(&format!("{wanted}-")) {
                let claimed = match claims.get(full) {
                    Some(product) => json!(product),
                    None => Value::Null,
                };
                taken.push(json!({"repository": full, "claimed_by": claimed}));
            }
        }
    }
    let free = taken.is_empty();
    if json {
        emit(&json!({"name": name, "free": free, "taken": taken}))?;
    } else if free {
        println!("{name} is free");
    } else {
        for row in &taken {
            match (row["held_by"].as_str(), row["claimed_by"].as_str()) {
                (Some(product), _) => {
                    println!("{name} is taken: the {} of {product}", field(row, "as"))
                }
                (None, Some(product)) => println!(
                    "{name} is taken: repository {} (claimed by {product})",
                    field(row, "repository")
                ),
                (None, None) => println!(
                    "{name} is taken: repository {} (claimed by no product)",
                    field(row, "repository")
                ),
            }
        }
    }
    Ok(if free {
        libc::EXIT_SUCCESS
    } else {
        libc::EXIT_FAILURE
    })
}

/// Remove a former name recorded in error. A name a product really had stays:
/// it is how a reader of an older page finds the product.
pub(super) fn forget(runtime: &Runtime, args: &Arguments, json: bool) -> Result<i32> {
    let [product] = args.positional.as_slice() else {
        bail!("name forget takes PRODUCT and FORMER");
    };
    let former = args.required("--former")?;
    let (_writer, original, mut document) = read_for_write(runtime)?;
    let index = position(&document, product, runtime)?;
    let wanted = key(former);
    let record = &mut document["products"][index];
    if key(text(record, "id")?) == wanted || key(text(record, "name")?) == wanted {
        bail!(
            "{former:?} is {product}'s current name, not a former one: a current name changes \
             with `stado product name set`; nothing was changed"
        );
    }
    let matches = |row: &Value| {
        ["id", "name"]
            .iter()
            .any(|name| row[*name].as_str().is_some_and(|word| key(word) == wanted))
    };
    let removed: Vec<Value> = former_rows(record)?
        .iter()
        .filter(|&row| matches(row))
        .cloned()
        .collect();
    if removed.is_empty() {
        let recorded: Vec<String> = former_rows(record)?
            .iter()
            .map(|row| format!("{} ({})", field(row, "id"), field(row, "name")))
            .collect();
        let recorded = if recorded.is_empty() {
            "none".to_owned()
        } else {
            recorded.join(", ")
        };
        bail!(
            "{product} has no former name {former:?}; its former names are {recorded}; nothing \
             was changed"
        );
    }
    let rows = record["former_names"]
        .as_array_mut()
        .context("former_names must be a list")?;
    rows.retain(|row| !matches(row));
    if rows.is_empty() {
        record
            .as_object_mut()
            .context("product record must be an object")?
            .remove("former_names");
    }
    let names = BTreeMap::from([(product.clone(), product.clone())]);
    crate::registry::document::replace_records(&runtime.catalog, &original, &document, &names)?;
    if json {
        let uncommitted = crate::catalog::committed::edit_visibility(&runtime.catalog)?;
        emit(&json!({"action": "forget", "product": product, "removed": removed,
            "written": [runtime.catalog], "uncommitted": uncommitted}))?;
    } else {
        println!(
            "{product}: forgot {} former name record(s) of {former}",
            removed.len()
        );
    }
    Ok(libc::EXIT_SUCCESS)
}
