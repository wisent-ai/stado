//! How `stado product dependencies` prints what it read: a table on stdout,
//! or the reports as JSON, and one refusal on stderr for each product it
//! cannot show running without another.

use anyhow::{Context, Result};
use serde_json::Value;

use super::{Found, Report};

/// The standing's own word, as its JSON names it.
fn word(found: &Found) -> Result<String> {
    let value = serde_json::to_value(&found.standing)?;
    value["standing"]
        .as_str()
        .map(str::to_owned)
        .context("a standing serializes with its word")
}

/// The channels a product is reached through, with how many lines each.
fn channels(found: &Found) -> Result<String> {
    let mut parts = Vec::new();
    for (channel, count) in &found.channels {
        let name = serde_json::to_value(channel)?;
        parts.push(format!(
            "{} {count}",
            name.as_str().context("a channel is a word")?
        ));
    }
    Ok(parts.join(", "))
}

/// Why `report`'s product is not shown to run alone, one sentence each.
fn refusals(report: &Report) -> Vec<String> {
    let mut said = Vec::new();
    for repository in report
        .repositories
        .iter()
        .filter(|repository| !repository.read)
    {
        said.push(format!(
            "{}: checkout {} of {} is absent, so what it calls is unknown",
            report.product, repository.checkout, repository.repository
        ));
    }
    for found in &report.dependencies {
        let Some(reason) = found.standing.refusal(&report.product, &found.product) else {
            continue;
        };
        let first = found.sites.first().map(|site| {
            format!(
                "; first reached at {}/{}:{}",
                site.repository, site.file, site.line
            )
        });
        said.push(format!(
            "{} cannot be shown to run without {}: {reason}{}",
            report.product,
            found.product,
            first.as_deref().map_or("", |text| text)
        ));
    }
    said
}

/// Print `reports` as a table, or as JSON when `json`, and every refusal on
/// stderr either way.
pub fn print(reports: &[Report], json: bool) -> Result<()> {
    if json {
        let value: Value = serde_json::to_value(reports)?;
        println!("{}", serde_json::to_string_pretty(&value)?);
    } else {
        for report in reports {
            let verdict = if report.independent {
                "independent"
            } else {
                "bound"
            };
            println!("{}\t{verdict}", report.product);
            for repository in &report.repositories {
                let state = if repository.read { "read" } else { "absent" };
                println!(
                    "  repository {}\t{state}\t{}",
                    repository.repository, repository.checkout
                );
            }
            for found in &report.dependencies {
                println!(
                    "  -> {}\t{}\t{}",
                    found.product,
                    word(found)?,
                    channels(found)?
                );
            }
        }
    }
    for report in reports {
        for refusal in refusals(report) {
            eprintln!("{refusal}");
        }
    }
    Ok(())
}
