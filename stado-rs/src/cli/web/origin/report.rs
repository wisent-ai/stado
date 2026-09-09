//! `stado web origin list` and `stado web origin status`.
//!
//! Status joins three independent readings, because they fail independently
//! and are repaired by different people: what the registry declares, what the
//! declared target publishes, and which origin the live public edge selected.
//! Collapsing any two of them would name the wrong repair — a funnel that is
//! on says nothing about a name that does not resolve, and a name that
//! resolves says nothing about an edge configured to fetch a different one.

use serde_json::{json, Value};

use super::verdict::{self, VERDICT_SERVING};
use crate::cli::CmdError;
use crate::public_origin::{self, PublicOrigin};
use crate::deploy::host_gates::observe;

pub(crate) async fn list(json_output: bool) -> Result<(), CmdError> {
    let (document, observation) = observe("registry", crate::targets::registry_location(),
        crate::cli::registry::fetch_document()).await;
    let document = document.ok_or_else(|| CmdError::click(
        observation.detail.unwrap_or_default()).machine_readable(json_output))?;
    let origins = public_origin::declarations(&document);
    if json_output {
        let rows: Vec<Value> = origins.iter().map(declaration_row).collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if origins.is_empty() {
        println!("no public origins are declared");
        return Ok(());
    }
    for origin in &origins {
        println!(
            "{} {} target={} publication={} upstream={} paths={}",
            origin.name,
            origin.origin(),
            origin.target,
            origin.publication,
            origin.upstream,
            origin.paths.join(",")
        );
    }
    Ok(())
}

pub(crate) fn declaration_row(origin: &PublicOrigin) -> Value {
    json!({
        "name": origin.name,
        "hostname": origin.hostname,
        "origin": origin.origin(),
        "target": origin.target,
        "publication": origin.publication,
        "upstream": origin.upstream,
        "paths": origin.paths,
    })
}

pub(crate) async fn status(name: Option<&str>, json_output: bool) -> Result<(), CmdError> {
    let ((document, registry_read), selection) = tokio::join!(
        observe("registry", crate::targets::registry_location(), crate::cli::registry::fetch_document()),
        verdict::edge_selection(),
    );
    let Some(document) = document else {
        let row = json!({
            "schema": "stado.public-origin-report.v1", "name": name,
            "origin": selection.origin, "complete": false,
            "verdict": "diagnostic-incomplete", "origin_error": registry_read.detail,
            "observations": [registry_read],
            "edge_selection": selection.report("unreadable"),
        });
        if json_output { println!("{}", serde_json::to_string_pretty(&[row])?); }
        else { print_row(&row); }
        return Err(CmdError::silent(1));
    };
    let registry = crate::targets::load_registry_from_value(&document)
        .map_err(|error| CmdError::click(error.to_string()).machine_readable(json_output))?;
    let declared = public_origin::declarations(&document);
    if let Some(wanted) = name {
        if !declared.iter().any(|origin| origin.name == wanted) {
            return Err(CmdError::usage(format!(
                "no public origin {wanted:?} is declared; declared: {}",
                declared_names(&declared)
            )));
        }
    }
    let mut rows = Vec::new();
    let mut broken = Vec::new();
    let examined = futures::future::join_all(declared.iter()
        .filter(|origin| name.is_none_or(|wanted| wanted == origin.name))
        .map(|origin| verdict::examine(origin, &selection, &registry))).await;
    for mut row in examined {
        row["registry_observation"] = json!(registry_read);
        let word = row["verdict"].as_str().unwrap_or("");
        if word != VERDICT_SERVING {
            broken.push(format!("{}: {word}", row["name"].as_str().unwrap_or("")));
        }
        rows.push(row);
    }
    // An origin the edge selected that no declaration names is the defect
    // this capability was built for, so it is reported as a row of its own
    // rather than left out of a report that would then look clean.
    if name.is_none() {
        if let Some(row) = verdict::undeclared_row(&declared, &selection) {
            let subject = match row["origin"].as_str() {
                Some(origin) if !origin.is_empty() => origin,
                _ => "the public origin boundary",
            };
            broken.push(format!(
                "{subject}: {}",
                row["verdict"].as_str().unwrap_or("")
            ));
            rows.push(row);
        }
    }
    if json_output {
        println!("{}", serde_json::to_string_pretty(&rows)?);
    } else {
        for row in &rows {
            print_row(row);
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

fn declared_names(declared: &[PublicOrigin]) -> String {
    if declared.is_empty() {
        return "none".to_string();
    }
    declared
        .iter()
        .map(|origin| origin.name.clone())
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_row(row: &Value) {
    let text = |key: &str| row[key].as_str().unwrap_or("");
    let nested = |section: &str, key: &str| row[section][key].as_str().unwrap_or("");
    println!(
        "{} {} {}",
        row["name"].as_str().unwrap_or("(undeclared)"),
        text("verdict"),
        text("origin")
    );
    println!(
        "  resolution:  {} — {}",
        nested("resolution", "state"),
        nested("resolution", "detail")
    );
    println!(
        "  publication: {} — {}",
        nested("publication_state", "state"),
        nested("publication_state", "detail")
    );
    println!(
        "  edge:        {} — {}",
        nested("edge_selection", "state"),
        nested("edge_selection", "detail")
    );
    if let Some(diagnosis) = row["edge_selection"].get("diagnosis").filter(|value| !value.is_null()) {
        println!("  origin diagnosis: {}", serde_json::to_string_pretty(diagnosis).expect("JSON value serializes"));
    }
    if let Some(reads) = row["observations"].as_array() {
        for read in reads {
            println!("  read {}: {} ({} ms) {} — {}",
                read["operation"].as_str().unwrap_or(""), read["state"].as_str().unwrap_or(""),
                read["elapsed_ms"], read["source"].as_str().unwrap_or(""),
                read["detail"].as_str().unwrap_or(""));
        }
    }
    if let Some(problem) = row["origin_error"].as_str() {
        println!("  origin:      {problem}");
    }
}
