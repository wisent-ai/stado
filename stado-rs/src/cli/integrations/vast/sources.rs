//! `stado market sources [--check] [--target HOST] [--json]`: every place a
//! fleet host can earn, from the declared list
//! `stado-rs/data/catalog/market-sources.json`, with whether Stado carries an
//! adapter for it.
//!
//! The list is data, not code, so a new marketplace or network is one row,
//! reviewed like any change. `--check` asks each source's page whether it
//! still answers and exits non-zero naming every one that does not, so a
//! source that closed or moved is reported instead of staying listed.
//! `--target HOST` adds what each source earned that host per board-hour
//! (see [`super::source_value`]) beside the best other source.

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

pub(crate) async fn sources(
    check: bool,
    target: Option<&str>,
    json_output: bool,
) -> Result<(), CmdError> {
    let valued = match target {
        None => None,
        Some(target) => {
            let registry = crate::cli::registry::read_registry().await?;
            let host = crate::cli::resolved_host(&registry, target)?.name.clone();
            let store = crate::queue::JobStorage::new().await?;
            Some(match super::source_value::window(&store).await {
                Err(cause) => Err(cause),
                Ok((since, seconds)) => Ok((
                    seconds,
                    super::source_value::values(&store, &host, since).await?,
                )),
            })
        }
    };
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
    let (read, unread) = match &valued {
        Some(Ok((seconds, values))) => (
            Some(json!({"window_seconds": seconds, "values": values})),
            None,
        ),
        Some(Err(cause)) => (None, Some(cause)),
        None => (None, None),
    };
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
                "target": target,
                "per_board_hour": read,
                "per_board_hour_unread": unread,
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
        match &valued {
            None => {}
            Some(Err(cause)) => println!("per board-hour: unread: {cause}"),
            Some(Ok((seconds, values))) => print_values(*seconds, values),
        }
    }
    match unreachable.is_empty() {
        true => Ok(()),
        false => Err(CmdError::silent(crate::cli::CLICK_ERROR_CODE)),
    }
}

/// Each source's value per board-hour beside the best other source's.
fn print_values(seconds: u64, values: &[super::source_value::SourceValue]) {
    println!("per board-hour over the last {seconds}s:");
    for value in values {
        let best_other = values
            .iter()
            .filter(|other| other.source != value.source)
            .filter_map(|other| other.usd_per_hour.map(|usd| (other, usd)))
            .max_by(|left, right| left.1.total_cmp(&right.1));
        let against = match best_other {
            Some((other, usd)) => format!("best other {} {usd:.4} USD", other.source),
            None => "no other source has a value".to_string(),
        };
        match (value.usd_per_hour, &value.unread) {
            (Some(usd), _) => println!(
                "{}\t{usd:.4} USD over {:.1}h\t{against}",
                value.source,
                value.hours.unwrap_or(0.0)
            ),
            (None, Some(cause)) => println!("{}\tunread: {cause}\t{against}", value.source),
            (None, None) => println!("{}\tunread\t{against}", value.source),
        }
    }
}
