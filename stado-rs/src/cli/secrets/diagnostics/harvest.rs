//! Credentials still recoverable from agent transcripts, and the single-name
//! restore path. Names and counts here, never a value.

use serde_json::{json, Value};

use crate::cli::{reporting::table, CmdError};

use crate::cli::secrets::diagnostics::doctor::key_doctor_report;
use crate::cli::secrets::store::resolve::{client, skarbiec_binary};

/// Inventory of credentials still recoverable from agent transcripts, and the
/// single-name restore path.
///
/// The report carries names, counts, dates and source files. It carries no
/// values, because the defect it measures is values reaching places that only
/// needed names — printing them here would add a terminal, a shell history and
/// this process's own transcript to that list. `--restore NAME` is the one path
/// a value travels, and it writes directly to the selected credential store.
pub(crate) async fn harvest(json: bool, restore: Option<&str>, all: bool) -> Result<(), CmdError> {
    if let Some(name) = restore {
        let value = crate::transcripts::value_for(name).ok_or_else(|| {
            CmdError::click(format!(
                "no secret-shaped value for {name} in any transcript; run without --restore to see what is there"
            ))
        })?;
        let selector = crate::credential_store::configured_selector()
            .map_err(|error| CmdError::click(error.to_string()))?;
        if selector.starts_with("skarbiec") {
            // Skarbiec can encrypt with public recipients even when no owner
            // here can decrypt. Refuse to bury the recovered value in that
            // state; other backends enforce their own write preconditions.
            let report = key_doctor_report(&skarbiec_binary()?)?;
            match report.get("status").and_then(Value::as_str) {
                Some("readable") | Some("empty") => {}
                _ => {
                    return Err(CmdError::click(format!(
                        "refusing to restore {name}: Skarbiec cannot be opened by any key here; own a readable vault first (see `stado secrets doctor`)"
                    )))
                }
            }
        }
        // One write path for every backend: a Skarbiec selector reaches the
        // vault through its owner inside the credential store, so this no
        // longer needs its own copy of that call.
        client()?
            .write_item(name, "stado-secret", &json!({"value": value}))
            .await
            .map_err(|error| CmdError::click(error.to_string()))?;
        println!("restored {name} into the selected credential store from transcript history");
        return Ok(());
    }
    let findings = crate::transcripts::scan(all);
    if json {
        let rendered: Vec<Value> = findings
            .iter()
            .map(|finding| {
                json!({
                    "name": finding.name,
                    "occurrences": finding.occurrences,
                    "distinct_values": finding.distinct_values,
                    "newest_seen": finding.newest_seen,
                    "sources": finding.sources.len(),
                    // The table shows this; automation needs it too, or it
                    // cannot tell a live credential from a committed literal.
                    "origin": match finding.origin {
                        crate::transcripts::Origin::Runtime => "runtime",
                        crate::transcripts::Origin::FileQuote => "file",
                    },
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&json!(rendered))?);
        return Ok(());
    }
    let rows: Vec<Vec<String>> = findings
        .iter()
        .map(|finding| {
            vec![
                finding.name.clone(),
                finding.occurrences.to_string(),
                finding.distinct_values.to_string(),
                finding.newest_seen.clone(),
                finding.sources.len().to_string(),
                match finding.origin {
                    crate::transcripts::Origin::Runtime => "runtime".to_string(),
                    crate::transcripts::Origin::FileQuote => "file".to_string(),
                },
            ]
        })
        .collect();
    table::print(
        &["NAME", "SEEN", "DISTINCT", "NEWEST", "FILES", "ORIGIN"],
        &rows,
    );
    println!(
        "{} recoverable credential name(s) in agent transcripts; restore one with --restore NAME",
        rows.len()
    );
    Ok(())
}
