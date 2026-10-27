mod document;
mod fields;
use crate::{
    catalog,
    common::{emit, lock, now, slug, Arguments, Runtime},
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    fs,
    io::{self, IsTerminal, Write},
};

pub fn run(action: &str, args: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let args = Arguments::from_matches(args);
    if action == "check-claims" {
        // Reads and fetches only: a claim whose source changed or cannot be
        // fetched fails the command after every claim was checked.
        let [id] = args.positional.as_slice() else {
            bail!("registry check-claims requires one product ID");
        };
        let (answer, current) = catalog::check_claims(runtime, id)?;
        emit(&answer)?;
        return Ok(if current {
            libc::EXIT_SUCCESS
        } else {
            libc::EXIT_FAILURE
        });
    }
    emit(&mutate(action, args, runtime)?)?;
    Ok(libc::EXIT_SUCCESS)
}

pub fn mutate(action: &str, args: Arguments, runtime: &Runtime) -> Result<Value> {
    let id = if action == "add" {
        if !args.positional.is_empty() {
            bail!("registry add names its product with --id");
        }
        args.required("--id")?
    } else {
        let [id] = args.positional.as_slice() else {
            bail!("registry {action} requires one product ID");
        };
        id.as_str()
    };
    slug(id)?;
    if action == "rm" && !args.has("--yes") {
        if !io::stdin().is_terminal() {
            bail!("removing {id} requires --yes when stdin is not a terminal");
        }
        eprint!("Remove {id} from {}? [y/N] ", runtime.catalog.display());
        io::stderr().flush()?;
        let mut answer = String::new();
        io::stdin().read_line(&mut answer)?;
        if answer.trim() != "y" && answer.trim() != "yes" {
            return Ok(json!({"action": "rm", "id": id, "written": [], "removed": false}));
        }
    }
    let _writer = lock(&runtime.catalog.with_extension("lock"))?;
    let original = fs::read_to_string(&runtime.catalog)?;
    let mut document: Value = serde_yaml::from_str(&original)?;
    catalog::validate(&document)?;
    let products = document["products"]
        .as_array_mut()
        .context("products must be a list")?;
    let position = products.iter().position(|product| product["id"] == id);
    if action == "rm" {
        products.remove(position.with_context(|| format!("no product {id}; nothing was removed"))?);
        document::commit(&runtime.catalog, &original, &document, id, true)?;
        return Ok(
            json!({"action": "rm", "id": id, "written": [runtime.catalog], "removed": true}),
        );
    }
    let mut record = match action {
        "add" => {
            if position.is_some() {
                bail!("product {id} already exists; use registry set");
            }
            json!({"id": id, "name": args.required("--name")?, "status": args.required("--status")?,
                "visibility": args.required("--visibility")?, "family": args.required("--family")?,
                "owner_repository": args.required("--owner-repository")?, "description": args.required("--description")?,
                "roadmap": [], "integrations": [], "evidence": args.many("--evidence"), "surfaces": [], "installations": []})
        }
        "set" => products
            [position.with_context(|| format!("no product {id}; nothing was changed"))?]
        .clone(),
        _ => bail!("unknown registry command {action}"),
    };
    set_field(&mut record, &args, "--name", "name")?;
    set_field(&mut record, &args, "--status", "status")?;
    set_field(&mut record, &args, "--visibility", "visibility")?;
    set_field(&mut record, &args, "--family", "family")?;
    set_field(&mut record, &args, "--owner-repository", "owner_repository")?;
    set_field(&mut record, &args, "--description", "description")?;
    if args.has("--evidence") {
        record["evidence"] = json!(args.many("--evidence"));
    }
    remove(
        &mut record,
        "surfaces",
        "kind",
        args.many("--remove-surface"),
    )?;
    for value in args
        .many("--surface")
        .iter()
        .chain(args.many("--add-surface"))
    {
        replace(&mut record, "surfaces", "kind", fields::surface(value)?)?;
    }
    remove(
        &mut record,
        "installations",
        "surface",
        args.many("--remove-installation"),
    )?;
    for value in args
        .many("--installation")
        .iter()
        .chain(args.many("--add-installation"))
    {
        let recipe = fields::installation(value, &record["surfaces"])?;
        replace(&mut record, "installations", "surface", recipe)?;
    }
    remove(
        &mut record,
        "integrations",
        "product",
        args.many("--remove-integration"),
    )?;
    for value in args
        .many("--integration")
        .iter()
        .chain(args.many("--add-integration"))
    {
        replace(
            &mut record,
            "integrations",
            "product",
            fields::integration(value)?,
        )?;
    }
    for (field, identity, removals, additions, parse) in [
        (
            "rivals",
            "id",
            "--remove-rival",
            "--add-rival",
            fields::rival as fn(&str) -> Result<Value>,
        ),
        (
            "roadmap",
            "title",
            "--remove-roadmap",
            "--add-roadmap",
            fields::roadmap,
        ),
    ] {
        if args.many(removals).is_empty() && args.many(additions).is_empty() {
            continue;
        }
        if record.get(field).is_none() {
            record[field] = json!([]);
        }
        remove(&mut record, field, identity, args.many(removals))?;
        for value in args.many(additions) {
            replace(&mut record, field, identity, parse(value)?)?;
        }
    }
    if record["rivals"].as_array().is_some_and(Vec::is_empty) {
        record
            .as_object_mut()
            .context("product record must be an object")?
            .remove("rivals");
    }
    let removals = args.many("--remove-dependency");
    let additions = args.many("--add-dependency");
    if !removals.is_empty() || !additions.is_empty() {
        if record.get("dependencies").is_none() {
            record["dependencies"] = json!([]);
        }
        remove_needs(&mut record, removals)?;
        for value in additions {
            replace_need(&mut record, fields::dependency(value)?)?;
        }
        if record["dependencies"].as_array().is_some_and(Vec::is_empty) {
            record
                .as_object_mut()
                .context("product record must be an object")?
                .remove("dependencies");
        }
    }
    if args.has("--remove-benchmark") {
        record
            .as_object_mut()
            .context("product record must be an object")?
            .remove("benchmark")
            .context("no benchmark is declared; nothing was changed")?;
    }
    if let Some(value) = args.optional("--benchmark")? {
        record["benchmark"] = fields::benchmark(value)?;
    }
    if let Some(file) = args.optional("--service-file")? {
        let service: Value = serde_yaml::from_slice(&fs::read(file)?)?;
        if !service.is_object() {
            bail!("service declaration must be an object");
        }
        record["service"] = service;
    }
    if let Some(approver) = args.optional("--approved-by")? {
        record["approved_by"] = json!(approver);
        record["approved_at"] = json!(now());
    }
    if let Some(note) = args.optional("--approval-note")? {
        if record.get("approved_by").is_none() {
            bail!("--approval-note requires --approved-by");
        }
        record["approval_note"] = json!(note);
    }
    if let Some(position) = position {
        products[position] = record.clone();
    } else {
        products.push(record.clone());
    }
    document::commit(&runtime.catalog, &original, &document, id, false)?;
    Ok(json!({"action": action, "id": id, "written": [runtime.catalog], "record": record}))
}

fn set_field(record: &mut Value, args: &Arguments, option: &str, field: &str) -> Result<()> {
    if let Some(value) = args.optional(option)? {
        record[field] = json!(value);
    }
    Ok(())
}

fn remove(record: &mut Value, field: &str, identity: &str, names: &[String]) -> Result<()> {
    let rows = record[field]
        .as_array_mut()
        .with_context(|| format!("{field} must be a list"))?;
    for name in names {
        let position = rows
            .iter()
            .position(|row| row[identity] == name.as_str())
            .with_context(|| format!("no {field} entry {name}; nothing was changed"))?;
        rows.remove(position);
    }
    Ok(())
}

fn replace(record: &mut Value, field: &str, identity: &str, row: Value) -> Result<()> {
    let rows = record[field]
        .as_array_mut()
        .with_context(|| format!("{field} must be a list"))?;
    if let Some(position) = rows.iter().position(|old| old[identity] == row[identity]) {
        rows[position] = row;
    } else {
        rows.push(row);
    }
    Ok(())
}

/// The fields that make one declared need of another product: the product,
/// its surface, and the surface of this product that needs it.
const NEED_IDENTITY: &[&str] = &["product", "surface", "for_surface"];

/// Remove every declared need of each product in `names`; a product with no
/// declared need is refused by name and nothing is written.
fn remove_needs(record: &mut Value, names: &[String]) -> Result<()> {
    let rows = record["dependencies"]
        .as_array_mut()
        .context("dependencies must be a list")?;
    for name in names {
        let before = rows.len();
        rows.retain(|row| row["product"] != name.as_str());
        if rows.len() == before {
            bail!("no dependency on {name} is declared; nothing was changed");
        }
    }
    Ok(())
}

/// Replace the declared need with the same product, surface and
/// `for_surface` as `row`, or add `row` when there is none.
fn replace_need(record: &mut Value, row: Value) -> Result<()> {
    let rows = record["dependencies"]
        .as_array_mut()
        .context("dependencies must be a list")?;
    let same = rows.iter().position(|old| {
        NEED_IDENTITY
            .iter()
            .all(|field| old.get(*field) == row.get(*field))
    });
    match same {
        Some(position) => rows[position] = row,
        None => rows.push(row),
    }
    Ok(())
}
