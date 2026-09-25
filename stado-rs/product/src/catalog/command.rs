use super::{current, rows, EMBEDDED};
use crate::common::{atomic_write, checked, emit, Arguments, Runtime};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeSet, fs, path::Path, process::Command};

pub fn cli_catalog(document: &Value) -> Result<Value> {
    let mut products = Vec::new();
    for product in document["products"]
        .as_array()
        .context("products must be a list")?
    {
        if let Some(surface) = product["surfaces"]
            .as_array()
            .context("surfaces must be a list")?
            .iter()
            .find(|s| s["kind"] == "cli")
        {
            let default_origin = format!("https://{}.wisent.com", super::text(product, "id")?);
            products.push(json!({"id": product["id"], "name": product["name"], "repository": surface["repository"],
                "installation": product["installations"].as_array().and_then(|recipes| recipes.iter().find(|r| r["surface"] == "cli")),
                "docs_origin": surface["docs_origin"].as_str().unwrap_or(&default_origin).trim_end_matches('/')}));
        }
    }
    Ok(json!({"products": products}))
}

pub fn services(document: &Value) -> Result<Value> {
    let mut services = Vec::new();
    for product in document["products"]
        .as_array()
        .context("products must be a list")?
    {
        let service = &product["service"];
        if service["installable"] != true {
            continue;
        }
        let mut row = json!({"name": product["id"], "summary": service["summary"], "program": service["program"], "args": service["args"]});
        if let Some(unit) = service.get("unit") {
            row["unit"] = unit.clone();
        }
        if let Some(env) = service.get("env") {
            row["env"] = env.clone();
        }
        if let Some(retired) = service.get("retired_units") {
            row["retired_units"] = retired.clone();
        }
        services.push(row);
    }
    Ok(json!({"services": services}))
}

pub fn claimed(document: &Value) -> Result<BTreeSet<String>> {
    let mut names = BTreeSet::new();
    for product in document["products"]
        .as_array()
        .context("products must be a list")?
    {
        names.insert(super::text(product, "owner_repository")?.to_owned());
        for surface in product["surfaces"]
            .as_array()
            .context("surfaces must be a list")?
        {
            names.insert(super::text(surface, "repository")?.to_owned());
        }
    }
    Ok(names)
}

pub fn run(args: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let args = Arguments::from_matches(args);
    if !args.positional.is_empty() {
        bail!("catalog does not take positional arguments");
    }
    let document = current(runtime)?;
    if args.has("--check-package") {
        if runtime.embedded_catalog {
            bail!("catalog authority {} is absent; an embedded catalog cannot independently check itself", runtime.catalog.display());
        }
        let embedded: Value = serde_yaml::from_str(EMBEDDED)?;
        if document != embedded {
            bail!(
                "embedded catalog differs from {}; rebuild the Rust package from this authority",
                runtime.catalog.display()
            );
        }
        return Ok(0);
    }
    if args.has("--unapproved") {
        emit(
            &json!({"products": document["products"].as_array().context("products must be a list")?.iter()
            .filter(|p| p.get("approved_by").is_none()).collect::<Vec<_>>()}),
        )?;
        return Ok(0);
    }
    if let Some(org) = args.optional("--unclaimed")? {
        let output = checked(Command::new("gh").args([
            "repo",
            "list",
            org,
            "--limit",
            "10000",
            "--json",
            "nameWithOwner,description,isArchived",
        ]))?;
        let repositories: Vec<Value> = serde_json::from_slice(&output.stdout)?;
        let known = claimed(&document)?;
        let unclaimed: Vec<_> = repositories
            .into_iter()
            .filter(|r| {
                r["isArchived"] != true
                    && r["nameWithOwner"]
                        .as_str()
                        .is_some_and(|name| !known.contains(name))
            })
            .collect();
        emit(&json!({"organization": org, "unclaimed": unclaimed}))?;
        return Ok(0);
    }
    if args.has("--check-repositories") {
        let mut problems = Vec::new();
        for name in claimed(&document)? {
            match checked(Command::new("gh").args([
                "repo",
                "view",
                &name,
                "--json",
                "nameWithOwner",
            ])) {
                Ok(output) => {
                    let result: Value = serde_json::from_slice(&output.stdout)?;
                    if result["nameWithOwner"].as_str() != Some(name.as_str()) {
                        problems
                            .push(json!({"repository": name, "actual": result["nameWithOwner"]}));
                    }
                }
                Err(error) => {
                    problems.push(json!({"repository": name, "error": error.to_string()}))
                }
            }
        }
        emit(&json!({"problems": problems}))?;
        return Ok(i32::from(!problems.is_empty()));
    }
    let result = if args.has("--cli-products") {
        cli_catalog(&document)?
    } else if args.has("--output") || args.has("--check") {
        services(&document)?
    } else {
        rows(&document)?
    };
    let mut bytes = serde_json::to_vec_pretty(&result)?;
    bytes.push(b'\n');
    if let Some(output) = args.optional("--output")? {
        atomic_write(Path::new(output), &bytes)?;
    } else if let Some(path) = args.optional("--check")? {
        let actual = fs::read(path).with_context(|| format!("reading generated catalog {path}"))?;
        if actual != bytes {
            bail!("{path}: differs from {}", runtime.catalog.display());
        }
    } else if args.has("--json") || args.has("--cli-products") {
        emit(&result)?;
    } else {
        for product in result["products"]
            .as_array()
            .context("products must be a list")?
        {
            println!(
                "{:<24} {:<12} {}",
                super::text(product, "id")?,
                super::text(product, "family")?,
                super::text(product, "name")?
            );
        }
    }
    Ok(0)
}
