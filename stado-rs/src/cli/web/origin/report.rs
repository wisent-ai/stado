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
use crate::cli::reporting::field;
use crate::cli::CmdError;
use crate::deploy::host_gates::observe;
use crate::public_origin::{self, PublicOrigin};

pub(crate) async fn list(json_output: bool) -> Result<(), CmdError> {
    let (document, observation) = observe(
        "registry",
        crate::targets::registry_location(),
        crate::cli::registry::fetch_document(),
    )
    .await;
    let document = document.ok_or_else(|| {
        // A reading that returned no document and named no cause is said as
        // such, not as a refusal with nothing in it.
        let detail = match observation.detail {
            Some(detail) => detail,
            None => {
                "the registry document was not read, and the reading named no cause".to_string()
            }
        };
        CmdError::click(detail).machine_readable(json_output)
    })?;
    let origins = public_origin::declarations(&document);
    let authors = last_changed(&origins).await;
    if json_output {
        let rows: Vec<Value> = origins
            .iter()
            .zip(&authors)
            .map(|(origin, author)| {
                let mut row = declaration_row(origin);
                row["last_changed"] = author.clone();
                row
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(());
    }
    if origins.is_empty() {
        println!("no public origins are declared");
        return Ok(());
    }
    for (origin, author) in origins.iter().zip(&authors) {
        println!(
            "{} {} target={} publication={} upstream={} paths={} last_changed={}",
            origin.name,
            origin.origin(),
            origin.target,
            origin.publication,
            origin.upstream,
            origin.paths.join(","),
            changed_by(author)
        );
    }
    Ok(())
}

/// The newest recorded write that changed each origin's declaration, as
/// `stado registry history --path public_origins.<name>` lists it; `null`
/// when no recorded write changed it, and `{"unread": <cause>}` when the
/// records cannot be listed. A declaration whose author nobody recorded is
/// said to be that, never attributed to the reader.
async fn last_changed(origins: &[PublicOrigin]) -> Vec<Value> {
    let store = match crate::targets::RegistryStore::open().await {
        Ok(store) => store,
        Err(error) => {
            let unread =
                json!({ "unread": format!("the registry store cannot be opened: {error}") });
            return vec![unread; origins.len()];
        }
    };
    let mut authors = Vec::with_capacity(origins.len());
    for origin in origins {
        let path = format!("{}.{}", public_origin::POLICY_KEY, origin.name);
        authors.push(match store.write_history(Some(&path)).await {
            // A record whose changed paths are unknown is listed by the
            // history because it cannot be ruled out; it is not named as
            // this origin's author, because it may not have touched it.
            Ok(records) => records
                .iter()
                .rev()
                .find(|record| record["changed"].is_array())
                .cloned()
                .unwrap_or(Value::Null),
            Err(error) => json!({ "unread": error.to_string() }),
        });
    }
    authors
}

/// One author record as the list line says it.
fn changed_by(record: &Value) -> String {
    if record.is_null() {
        return "unrecorded (written before Stado recorded registry authors)".to_string();
    }
    if let Some(cause) = record["unread"].as_str() {
        return format!("unread ({cause})");
    }
    let by = &record["by"];
    let actor = by["actor"].as_str().unwrap_or("unread account");
    let host = by["host"].as_str().unwrap_or("unread host");
    let session = match by["session"].as_str() {
        Some(session) => format!(" session {session}"),
        None => String::new(),
    };
    format!(
        "{actor}@{host}{session} at {}",
        record["recorded_at"].as_str().unwrap_or("an unread time")
    )
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
        observe(
            "registry",
            crate::targets::registry_location(),
            crate::cli::registry::fetch_document()
        ),
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
        if json_output {
            println!("{}", serde_json::to_string_pretty(&[row])?);
        } else {
            print_row(&row);
        }
        return Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE));
    };
    let registry = crate::targets::load_registry_from_value(&document)
        .map_err(|error| CmdError::from(error).machine_readable(json_output))?;
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
    let shown: Vec<PublicOrigin> = declared
        .iter()
        .filter(|origin| name.is_none_or(|wanted| wanted == origin.name))
        .cloned()
        .collect();
    let (examined, authors) = tokio::join!(
        futures::future::join_all(
            shown
                .iter()
                .map(|origin| verdict::examine(origin, &selection, &registry)),
        ),
        last_changed(&shown),
    );
    for (mut row, author) in examined.into_iter().zip(authors) {
        row["registry_observation"] = json!(registry_read);
        row["last_changed"] = author;
        let word = field(&row["verdict"]);
        if row["verdict"].as_str() != Some(VERDICT_SERVING) {
            broken.push(format!("{}: {word}", field(&row["name"])));
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
            broken.push(format!("{subject}: {}", field(&row["verdict"])));
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
        return Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE));
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
    let text = |key: &str| field(&row[key]);
    let nested = |section: &str, key: &str| field(&row[section][key]);
    println!("{} {} {}", text("name"), text("verdict"), text("origin"));
    if row.get("last_changed").is_some() {
        println!("  last changed: {}", changed_by(&row["last_changed"]));
    }
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
    if let Some(diagnosis) = row["edge_selection"]
        .get("diagnosis")
        .filter(|value| !value.is_null())
    {
        match serde_json::to_string_pretty(diagnosis) {
            Ok(diagnosis) => println!("  origin diagnosis: {diagnosis}"),
            Err(error) => println!("  origin diagnosis: cannot be written out: {error}"),
        }
    }
    if let Some(reads) = row["observations"].as_array() {
        for read in reads {
            println!(
                "  read {}: {} ({} ms) {} — {}",
                field(&read["operation"]),
                field(&read["state"]),
                field(&read["elapsed_ms"]),
                field(&read["source"]),
                field(&read["detail"])
            );
        }
    }
    if let Some(problem) = row["origin_error"].as_str() {
        println!("  origin:      {problem}");
    }
}

/// `stado web origin url PATH [--query KEY=VALUE]...`: the public URL of
/// `PATH` on the one declared origin that publishes it, so a product stamps
/// the address its clients read from the declaration instead of a hostname
/// written into its own scripts. Refused when no origin publishes the path,
/// or when several do and the choice would be arbitrary.
pub(crate) async fn url(path: &str, query: &[String]) -> Result<(), CmdError> {
    let document = crate::cli::registry::fetch_document().await?;
    let origin = public_origin::publishing(&document, path).map_err(CmdError::declaration)?;
    let mut address = url::Url::parse(&format!("{}{path}", origin.origin())).map_err(|error| {
        CmdError::click(format!("{}{path}: {error}", origin.origin()))
            .stating(crate::primitives::failure::FailureCode::Config)
    })?;
    for pair in query {
        let (key, value) = pair
            .split_once('=')
            .ok_or_else(|| CmdError::usage(format!("--query {pair:?} is not KEY=VALUE")))?;
        address.query_pairs_mut().append_pair(key, value);
    }
    println!("{address}");
    Ok(())
}
