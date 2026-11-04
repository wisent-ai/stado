//! `stado market sources [--check] [--json]`: every place a fleet host can
//! earn, from the declared list `stado-rs/data/catalog/market-sources.json`,
//! with whether Stado carries an adapter for it.
//!
//! The list is data, not code, so a new marketplace or network is one row,
//! reviewed like any change. `--check` asks each source's page whether it
//! still answers and exits non-zero naming every one that does not, so a
//! source that closed or moved is reported instead of staying listed.

use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::cli::CmdError;

/// Where the list is declared, for the messages that name it.
const DECLARATION: &str = "stado-rs/data/catalog/market-sources.json";

#[derive(Debug, Clone, Deserialize, Serialize)]
struct Source {
    id: String,
    name: String,
    kind: String,
    unit: String,
    currency: String,
    join: String,
    adapter: Option<String>,
}

#[derive(Deserialize)]
struct Document {
    sources: Vec<Source>,
}

fn declared() -> Result<Vec<Source>, CmdError> {
    let document: Document =
        serde_json::from_str(include_str!("../../../../data/catalog/market-sources.json"))
            .map_err(|error| {
                CmdError::declaration(format!("{DECLARATION} does not parse: {error}"))
            })?;
    Ok(document.sources)
}

/// Every source id the list declares, for a command that names a source.
pub(crate) fn source_ids() -> Result<Vec<String>, CmdError> {
    Ok(declared()?.into_iter().map(|source| source.id).collect())
}

/// Whether `url` answers with a status below 400, or why not.
async fn answers(client: &reqwest::Client, url: &str) -> Result<(), String> {
    let response = crate::wait::request(client.get(url))
        .await
        .map_err(|error| format!("no answer: {error}"))?;
    let status = response.status();
    if status.as_u16() >= 400 {
        return Err(format!("answered {status}"));
    }
    Ok(())
}

pub(crate) async fn sources(check: bool, json_output: bool) -> Result<(), CmdError> {
    let sources = declared()?;
    let mut unreachable: Vec<(String, String)> = Vec::new();
    if check {
        let client = reqwest::Client::builder()
            .user_agent("stado-market-sources/1")
            .build()
            .map_err(|error| CmdError::click(format!("no HTTP client: {error}")))?;
        for source in &sources {
            if let Err(cause) = answers(&client, &source.join).await {
                unreachable.push((source.id.clone(), format!("{}: {cause}", source.join)));
            }
        }
    }
    if json_output {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "schema": "stado.market-sources.v1",
                "declaration": DECLARATION,
                "sources": sources,
                "checked": check,
                "unreachable": unreachable
                    .iter()
                    .map(|(id, cause)| json!({"id": id, "cause": cause}))
                    .collect::<Vec<_>>(),
            }))?
        );
    } else {
        println!("ID\tKIND\tUNIT\tCURRENCY\tADAPTER\tNAME");
        for source in &sources {
            println!(
                "{}\t{}\t{}\t{}\t{}\t{}",
                source.id,
                source.kind,
                source.unit,
                source.currency,
                source.adapter.as_deref().unwrap_or("none yet"),
                source.name
            );
        }
        for (id, cause) in &unreachable {
            println!("unreachable: {id}: {cause}");
        }
    }
    match unreachable.is_empty() {
        true => Ok(()),
        false => Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE)),
    }
}
