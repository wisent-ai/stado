//! `stado product name set PRODUCT`: one naming decision, recorded and
//! carried through the catalog.
//!
//! The decision names who decided it and why (`--decided-by`, `--reason`),
//! and the new id (`--id`), the new display name (`--name`) or both. The
//! record keeps it as `naming`, and the names it had before become a row of
//! `former_names`. A new id is carried into every record that names the
//! product: other products' integrations and dependencies. Declarations
//! whose value is a name another system already holds stay what they were,
//! made explicit where they were implied by the id: the service unit label
//! (`service.unit`, the launchd or systemd label hosts run), the service
//! directory key (`service.directory_service`), the name its releases are
//! published under (`installations[].release_product` of a `stado-release`
//! recipe) and the site its CLI documentation is published at (the cli
//! surface's `docs_origin`). So a renamed product keeps installing from its
//! published releases, keeps its running service and its documentation, and
//! each of those moves to the new id when its own owner moves it.
//!
//! The command then moves this machine's installation receipts to the new id
//! and reads, without changing anything, what outside the catalog still
//! carries the former id: the repositories, the release manifests and the
//! product's documentation site. Each is owned by its own system (GitHub, the
//! product's repository, the site), and the answer names each one with the
//! command that moves it.

use super::{holder, key, outside, position, read_for_write, receipts};
use crate::{
    catalog::text,
    common::{emit, now, slug, Arguments, Runtime},
    registry,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// The service unit label a product's service runs under when its record
/// declares none: the prefix the catalog validation requires, and its id.
const UNIT_PREFIX: &str = "com.wisent.";

pub(super) fn set(runtime: &Runtime, args: &Arguments, json: bool) -> Result<i32> {
    let [product] = args.positional.as_slice() else {
        bail!("name set takes one PRODUCT");
    };
    let product = product.as_str();
    let new_id = args.optional("--id")?;
    let new_name = args.optional("--name")?;
    if new_id.is_none() && new_name.is_none() {
        bail!(
            "name set {product} names nothing to change: give --id NEW-ID, --name \"New Name\" or \
             both; nothing was changed"
        );
    }
    let decided_by = args.required("--decided-by")?.trim();
    let reason = args.required("--reason")?.trim();
    if decided_by.is_empty() || reason.is_empty() {
        bail!(
            "--decided-by and --reason must say who decided the name and why; nothing was changed"
        );
    }
    if let Some(id) = new_id {
        slug(id).with_context(|| format!("--id {id:?} cannot be a product id; nothing was changed"))?;
    }
    let (_writer, original, mut document) = read_for_write(runtime)?;
    let index = position(&document, product, runtime)?;
    let current_name = text(&document["products"][index], "name")?.to_owned();
    let target_id = match new_id {
        Some(id) => id,
        None => product,
    };
    let target_name = match new_name {
        Some(name) => name.trim(),
        None => current_name.as_str(),
    };
    if target_name.is_empty() {
        bail!("--name must not be empty; nothing was changed");
    }
    if target_id == product && target_name == current_name {
        bail!("{product} is already called {product} ({current_name}); nothing was changed");
    }
    for (word, what) in [(target_id, "--id"), (target_name, "--name")] {
        if let Some(held) = holder(&document, word)? {
            if held.id != product {
                bail!(
                    "{what} {word:?} is already the {} of {}: a name, current or former, \
                     belongs to one product, so choose another (`stado product name check \
                     NAME` says whether a name is free); nothing was changed",
                    held.held_as,
                    held.id
                );
            }
        }
    }
    let decided_at = now();
    let mut kept = Vec::new();
    let record = &mut document["products"][index];
    let former = former_row(record, product, &current_name, &decided_at);
    let object = record
        .as_object_mut()
        .context("product record must be an object")?;
    if target_id != product {
        kept = keep_declarations(object, product)?;
        object.insert("id".to_owned(), json!(target_id));
    }
    object.insert("name".to_owned(), json!(target_name));
    object.insert(
        "naming".to_owned(),
        json!({"decided_by": decided_by, "decided_at": decided_at, "reason": reason}),
    );
    let rows = object
        .entry("former_names")
        .or_insert_with(|| json!([]))
        .as_array_mut()
        .context("former_names must be a list")?;
    // A name taken back is current again, so the row that recorded it as
    // former goes; a row for the names being closed is written once.
    let same = |row: &Value, id: &str, name: &str| {
        row["id"] == id && row["name"].as_str().is_some_and(|word| key(word) == key(name))
    };
    rows.retain(|row| !same(row, target_id, target_name) && !same(row, product, &current_name));
    rows.push(former);
    let mut records = BTreeMap::from([(product.to_owned(), target_id.to_owned())]);
    let mut references = Vec::new();
    if target_id != product {
        references = carry_references(&mut document, product, target_id)?;
        for reference in &references {
            let owner = reference["product"]
                .as_str()
                .context("a carried reference names its product")?;
            records.insert(owner.to_owned(), owner.to_owned());
        }
    }
    registry::document::replace_records(&runtime.catalog, &original, &document, &records)?;
    let moved = if target_id == product {
        Vec::new()
    } else {
        receipts::rename(runtime, product, target_id)?
    };
    let renamed = crate::catalog::product(&document, target_id)?;
    let outside = if target_id == product {
        Vec::new()
    } else {
        outside::read(runtime, renamed, product, target_id, args.optional("--org")?)?
    };
    // The decision is in the working tree; readers read the committed
    // catalog, so the answer says what commits it.
    let uncommitted = crate::catalog::committed::edit_visibility(&runtime.catalog)?;
    let answer = json!({
        "action": "set",
        "product": target_id,
        "former": {"id": product, "name": current_name},
        "name": target_name,
        "naming": renamed["naming"],
        "written": [runtime.catalog],
        "uncommitted": uncommitted,
        "references": references,
        "kept": kept,
        "receipts": moved,
        "outside": outside,
    });
    if json {
        emit(&answer)?;
    } else {
        print(&answer);
    }
    Ok(libc::EXIT_SUCCESS)
}

/// The names `record` had, as the `former_names` row this decision closes,
/// with the decision they stood under when the record has one.
fn former_row(record: &Value, id: &str, name: &str, until: &str) -> Value {
    let mut row = Map::new();
    row.insert("id".to_owned(), json!(id));
    row.insert("name".to_owned(), json!(name));
    row.insert("until".to_owned(), json!(until));
    if let Some(decision) = record.get("naming") {
        for field in ["decided_by", "decided_at", "reason"] {
            row.insert(field.to_owned(), decision[field].clone());
        }
    }
    Value::Object(row)
}

/// Make explicit, under the former id, every declaration a published release
/// or a running service still answers to, and return what was kept.
fn keep_declarations(record: &mut Map<String, Value>, former: &str) -> Result<Vec<Value>> {
    let mut kept = Vec::new();
    if let Some(service) = record.get_mut("service").and_then(Value::as_object_mut) {
        if !service.contains_key("unit") {
            let unit = format!("{UNIT_PREFIX}{former}");
            service.insert("unit".to_owned(), json!(unit));
            kept.push(json!({"field": "service.unit", "value": unit,
                "because": "hosts run the service under this launchd or systemd label"}));
        }
        if !service.contains_key("directory_service") {
            service.insert("directory_service".to_owned(), json!(former));
            kept.push(json!({"field": "service.directory_service", "value": former,
                "because": "the service directory and its consumers find the service under this key"}));
        }
    }
    if let Some(installations) = record.get_mut("installations").and_then(Value::as_array_mut) {
        for installation in installations {
            if installation["kind"] != "stado-release" || installation.get("release_product").is_some() {
                continue;
            }
            let Some(surface) = installation["surface"].as_str().map(str::to_owned) else {
                bail!("an installation of {former} names no surface");
            };
            installation["release_product"] = json!(former);
            kept.push(json!({"field": format!("installations.{surface}.release_product"), "value": former,
                "because": "its releases are published as this product, and its release manifest declares it"}));
        }
    }
    if let Some(surfaces) = record.get_mut("surfaces").and_then(Value::as_array_mut) {
        for surface in surfaces {
            if surface["kind"] != "cli" || surface.get("docs_origin").is_some() {
                continue;
            }
            let origin = crate::catalog::docs_origin(surface, former);
            surface["docs_origin"] = json!(origin);
            kept.push(json!({"field": "surfaces.cli.docs_origin", "value": origin,
                "because": "its CLI documentation is published at this site"}));
        }
    }
    Ok(kept)
}

/// Rewrite every other record's integration and dependency on `former` to
/// `id`, and return each one changed.
fn carry_references(document: &mut Value, former: &str, id: &str) -> Result<Vec<Value>> {
    let mut carried = Vec::new();
    for product in document["products"]
        .as_array_mut()
        .context("products must be a list")?
    {
        let owner = text(product, "id")?.to_owned();
        if owner == id {
            continue;
        }
        for field in ["integrations", "dependencies"] {
            let Some(rows) = product.get_mut(field).and_then(Value::as_array_mut) else {
                continue;
            };
            for row in rows {
                if row["product"] == former {
                    row["product"] = json!(id);
                    carried.push(json!({"product": owner, "field": field}));
                }
            }
        }
    }
    Ok(carried)
}

fn print(answer: &Value) {
    let field = |value: &Value| match value.as_str() {
        Some(text) => text.to_owned(),
        None => value.to_string(),
    };
    let rows = |name: &str| answer[name].as_array().into_iter().flatten();
    let (former, naming) = (&answer["former"], &answer["naming"]);
    println!(
        "{} ({}) is now {} ({}); decided by {} at {}: {}",
        field(&former["id"]), field(&former["name"]), field(&answer["product"]),
        field(&answer["name"]), field(&naming["decided_by"]), field(&naming["decided_at"]),
        field(&naming["reason"]),
    );
    for row in rows("references") {
        let (owner, kind) = (field(&row["product"]), field(&row["field"]));
        println!("  {owner}.{kind} now names {}", field(&answer["product"]));
    }
    for row in rows("kept") {
        let (name, value) = (field(&row["field"]), field(&row["value"]));
        println!("  kept {name} = {value}: {}", field(&row["because"]));
    }
    for row in rows("receipts") {
        println!("  moved receipt {} -> {}", field(&row["from"]), field(&row["to"]));
    }
    for row in rows("outside") {
        let (state, what) = (field(&row["state"]), field(&row["what"]));
        println!("  still {state}: {what} ({})", field(&row["next"]));
    }
    let uncommitted = &answer["uncommitted"];
    if uncommitted.is_object() {
        let readers = field(&uncommitted["readers"]);
        println!("  readers {readers}: {}", field(&uncommitted["commit"]));
    }
}
