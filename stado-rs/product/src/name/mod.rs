//! `stado product name`: where a product's names are decided.
//!
//! A product has an id (the word every program, receipt and route keys it
//! by) and a display name. Both live in its catalog record, and so does the
//! decision that gave them (`naming`: who decided, when, why) and every name
//! it had before (`former_names`). `set` is the one operation that changes
//! them: it records the decision, carries a new id into every catalog record
//! that names the product, keeps the declarations a published release or a
//! running service still answers to under the former id, moves this
//! machine's installation receipts, and names what outside the catalog still
//! carries the former id. `show` answers which product a name, current or
//! former, belongs to; `check` whether a name is free; `forget` removes a
//! former name recorded in error; `adopt` moves receipts left under a former
//! id on any machine.

mod check;
mod decide;
mod outside;
pub(crate) mod receipts;

use crate::{
    catalog::{self, text},
    common::{emit, lock, Arguments, Runtime},
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::fs;

/// The key a name is compared under: a reader does not tell `Echo` from
/// `echo`, so neither does the catalog.
pub(crate) fn key(name: &str) -> String {
    name.trim().to_lowercase()
}

pub fn run(action: &str, args: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let args = Arguments::from_matches(args);
    let json = args.has("--json");
    match action {
        "list" => list(runtime, json),
        "show" => show(runtime, &args, json),
        "check" => check::check(runtime, &args, json),
        "set" => decide::set(runtime, &args, json),
        "forget" => check::forget(runtime, &args, json),
        "adopt" => receipts::command(runtime, json),
        other => bail!("unknown name operation {other}"),
    }
}

/// How one product holds a name.
pub(crate) struct Held<'a> {
    pub product: &'a Value,
    pub id: &'a str,
    pub held_as: &'static str,
    pub former: Option<&'a Value>,
}

pub(crate) fn products(document: &Value) -> Result<&Vec<Value>> {
    document["products"]
        .as_array()
        .context("products must be a list")
}

/// The names a product had before, oldest first.
pub(crate) fn former_rows(product: &Value) -> Result<&[Value]> {
    match product.get("former_names") {
        None => Ok(&[]),
        Some(rows) => Ok(rows
            .as_array()
            .context("former_names must be a list")?
            .as_slice()),
    }
}

/// The product that holds `name` as its id, display name or a former name.
pub(crate) fn holder<'a>(document: &'a Value, name: &str) -> Result<Option<Held<'a>>> {
    let wanted = key(name);
    let products = products(document)?;
    for product in products {
        let id = text(product, "id")?;
        for (word, held_as) in [(id, "id"), (text(product, "name")?, "display name")] {
            if key(word) == wanted {
                return Ok(Some(Held { product, id, held_as, former: None }));
            }
        }
    }
    for product in products {
        let id = text(product, "id")?;
        for row in former_rows(product)? {
            for (field, held_as) in [("id", "former id"), ("name", "former display name")] {
                if key(text(row, field)?) == wanted {
                    return Ok(Some(Held { product, id, held_as, former: Some(row) }));
                }
            }
        }
    }
    Ok(None)
}

/// The position of product `id`; a former name or an unknown word is
/// refused naming what it is.
pub(crate) fn position(document: &Value, id: &str, runtime: &Runtime) -> Result<usize> {
    if let Some(index) = products(document)?.iter().position(|product| product["id"] == id) {
        return Ok(index);
    }
    match holder(document, id)? {
        Some(held) => bail!(
            "{id} is the {} of {}, not a product id: name the product as {}; nothing was changed",
            held.held_as,
            held.id,
            held.id
        ),
        None => bail!(
            "no product {id} in {}: `stado product name list` lists every product; nothing was \
             changed",
            runtime.catalog.display()
        ),
    }
}

/// The catalog file read whole for a write, with the writer lock held. A
/// name is decided in the catalog file; the copy compiled into Stado is not
/// one.
pub(crate) fn read_for_write(runtime: &Runtime) -> Result<(fs::File, String, Value)> {
    if runtime.embedded_catalog {
        bail!(
            "{} does not exist: a name is decided in the catalog file, so run this where the \
             workspace holds stado/catalog/products.yml (--workspace), or name the file with \
             --catalog; nothing was changed",
            runtime.catalog.display()
        );
    }
    let writer = lock(&runtime.catalog.with_extension("lock"))?;
    let original = fs::read_to_string(&runtime.catalog)
        .with_context(|| format!("reading the catalog {}", runtime.catalog.display()))?;
    let document: Value = serde_yaml::from_str(&original)
        .with_context(|| format!("parsing the catalog {}", runtime.catalog.display()))?;
    catalog::validate(&document)?;
    Ok((writer, original, document))
}

/// A string field of a validated row, which validation guarantees.
fn field<'a>(row: &'a Value, name: &str) -> &'a str {
    match row[name].as_str() {
        Some(text) => text,
        None => "(not recorded)",
    }
}

fn row(product: &Value) -> Result<Value> {
    let naming = match product.get("naming") {
        Some(decision) => decision.clone(),
        None => Value::Null,
    };
    Ok(json!({"id": text(product, "id")?, "name": text(product, "name")?,
        "naming": naming, "former_names": former_rows(product)?}))
}

/// One product's names as a person reads them.
fn lines(row: &Value) -> String {
    let naming = &row["naming"];
    let decision = if naming.is_object() {
        format!(
            "decided by {} at {}: {}",
            field(naming, "decided_by"),
            field(naming, "decided_at"),
            field(naming, "reason")
        )
    } else {
        "no recorded decision".to_owned()
    };
    let mut text = format!(
        "{:<24} {:<28} {decision}",
        field(row, "id"),
        field(row, "name")
    );
    for former in row["former_names"].as_array().into_iter().flatten() {
        text.push_str(&format!(
            "\n    was {} ({}) until {}",
            field(former, "id"),
            field(former, "name"),
            field(former, "until")
        ));
    }
    text
}

fn list(runtime: &Runtime, json: bool) -> Result<i32> {
    let document = catalog::current(runtime)?;
    let rows = products(&document)?
        .iter()
        .map(row)
        .collect::<Result<Vec<_>>>()?;
    if json {
        emit(&json!({"products": rows}))?;
    } else {
        for row in &rows {
            println!("{}", lines(row));
        }
    }
    Ok(libc::EXIT_SUCCESS)
}

fn show(runtime: &Runtime, args: &Arguments, json: bool) -> Result<i32> {
    let [name] = args.positional.as_slice() else {
        bail!("name show takes one NAME");
    };
    let document = catalog::current(runtime)?;
    let Some(held) = holder(&document, name)? else {
        bail!(
            "no product is or was called {name:?} in {}: `stado product name list` lists every \
             name, current and former",
            runtime.catalog.display()
        );
    };
    let record = row(held.product)?;
    let answer = json!({"name": name, "product": held.id, "as": held.held_as,
        "former": held.former, "record": record});
    if json {
        emit(&answer)?;
    } else {
        println!("{name} is the {} of {}", held.held_as, held.id);
        println!("{}", lines(&record));
    }
    Ok(libc::EXIT_SUCCESS)
}
