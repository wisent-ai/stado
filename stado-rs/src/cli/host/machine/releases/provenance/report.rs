use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::machine::releases::provenance::{CarriedArtifact, READ_PROVENANCE_BODY};

/// `stado release provenance --host TARGET [--json]` — what TARGET carries,
/// and who produced it.
///
/// The command that did not exist on 2026-08-11, when the only record of what
/// was running the control plane was a version string the repository had never
/// heard of. Every artifact under the host's Stado bin directory gets a row,
/// whether or not anything accounts for it, and an artifact with no manifest
/// is reported `unprovenanced` -- absent from the table is the one outcome
/// this must never produce, because that is precisely what the fleet did for
/// months.
///
/// Reachability is resolved here rather than read from the manifest, against a
/// checkout this process can see, because it is a question whose answer
/// changes: a commit unreachable at install time becomes reachable the moment
/// someone pushes, and a stored verdict would still be accusing them.
pub async fn provenance(target: &str, json: bool) -> Result<(), CmdError> {
    let resolved = crate::deploy::host_channel::canonical_target(target)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    let runner = crate::deploy::production_runner();
    let script = format!("set -euo pipefail\n{READ_PROVENANCE_BODY}");
    let output = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(|error| CmdError::click(error.to_string()))?;
    if !output.ok() {
        return Err(CmdError::click(format!(
            "{target}: cannot read provenance manifests: {}",
            crate::deploy::host_channel::last_error_line(&output, "remote provenance read failed")
        )));
    }

    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut records: std::collections::BTreeMap<String, crate::provenance::Provenance> =
        std::collections::BTreeMap::new();
    let mut unreadable: Vec<String> = Vec::new();
    let mut helpers: usize = 0;
    let mut present: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    for line in output.stdout.lines() {
        if let Some(artifact) = line.strip_prefix("STADO-ARTIFACT ") {
            // `<kind> <digest> <name>`. A helper script has no release behind
            // it, so listing it beside the control-plane binary answers a
            // question nobody asked and hides the one that matters.
            let mut words = artifact.trim().splitn(3, ' ');
            let kind = words.next().unwrap_or_default();
            let digest = words.next().unwrap_or_default().trim().to_string();
            let Some(name) = words.next().map(str::trim).filter(|name| !name.is_empty()) else {
                continue;
            };
            if kind == "script" {
                helpers += 1;
                continue;
            }
            if !digest.is_empty() && digest != "-" {
                present.insert(name.to_string(), digest);
            }
            names.insert(name.to_string());
        } else if let Some(document) = line.strip_prefix("STADO-MANIFEST ") {
            match serde_json::from_str::<crate::provenance::Provenance>(document.trim()) {
                Ok(record) => {
                    names.insert(record.artifact.clone());
                    records.insert(record.artifact.clone(), record);
                }
                // A manifest that cannot be parsed is not a missing manifest:
                // something wrote a file there and it says nothing usable.
                Err(error) => unreadable.push(error.to_string()),
            }
        }
    }

    let repository = crate::provenance::local_repo();
    let now = chrono::Utc::now();
    let carried: Vec<CarriedArtifact> = names
        .into_iter()
        .map(|artifact| {
            let record = records.remove(&artifact);
            let reachable = match (&record, &repository) {
                (None, _) => Some(false),
                (Some(record), _) if !record.names_a_commit() => Some(false),
                (Some(_), None) => None,
                (Some(record), Some(repository)) => Some(crate::provenance::reachable_in_repo(
                    &record.commit,
                    repository,
                )),
            };
            let age_seconds = record.as_ref().and_then(|record| {
                chrono::DateTime::parse_from_rfc3339(&record.at)
                    .ok()
                    .map(|stamp| (now - stamp.with_timezone(&chrono::Utc)).num_seconds())
            });
            let describes = match (&record, present.get(&artifact)) {
                (Some(record), Some(actual)) => Some(record.sha256.eq_ignore_ascii_case(actual)),
                _ => None,
            };
            CarriedArtifact {
                artifact,
                record,
                reachable,
                describes,
                age_seconds,
            }
        })
        .collect();

    let commit_of = |item: &CarriedArtifact| {
        item.record.as_ref().map_or_else(
            || crate::provenance::UNPROVENANCED.to_string(),
            |record| record.commit.clone(),
        )
    };
    let drifted = carried
        .iter()
        .filter(|item| item.reachable != Some(true))
        .count();

    if json {
        let artifacts: Vec<Value> = carried
            .iter()
            .map(|item| {
                json!({
                    "artifact": item.artifact,
                    "manifest": item.record.is_some(),
                    "commit": commit_of(item),
                    "sha256": item.record.as_ref().map(|record| record.sha256.clone()),
                    "builder": item.record.as_ref().map(|record| record.builder.clone()),
                    "at": item.record.as_ref().map(|record| record.at.clone()),
                    "age_seconds": item.age_seconds,
                    "reachable": item.reachable,
                    "describes_artifact": item.describes,
                })
            })
            .collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "target": target,
                "repository": repository.as_ref().map(|path| path.display().to_string()),
                "artifacts": artifacts,
                "unreadable_manifests": unreadable,
                "drifted": drifted,
            }))?
        );
        return Ok(());
    }

    let rows: Vec<Vec<String>> = carried
        .iter()
        .map(|item| {
            let age = match (item.age_seconds, &item.record) {
                (Some(seconds), _) => {
                    crate::cli::registry::human_age(chrono::TimeDelta::seconds(seconds))
                }
                // A manifest whose timestamp will not parse is a manifest
                // somebody hand-edited; say so instead of showing an age.
                (None, Some(_)) => "unknown".to_string(),
                (None, None) => "never".to_string(),
            };
            let reachable = match item.reachable {
                Some(true) => "yes",
                Some(false) => "no",
                None => "unknown",
            };
            let describes = match item.describes {
                Some(true) => "match",
                Some(false) => "REPLACED",
                None => "-",
            };
            vec![
                item.artifact.clone(),
                commit_of(item),
                item.record
                    .as_ref()
                    .map_or_else(|| "-".to_string(), |record| record.builder.clone()),
                age,
                reachable.to_string(),
                describes.to_string(),
            ]
        })
        .collect();
    // Before the early return as well as before the table: a host whose only
    // provenance file is corrupt must not read as a host with nothing to say.
    for error in &unreadable {
        eprintln!("{target}: a provenance manifest could not be read: {error}");
    }
    if rows.is_empty() {
        println!("{target}: carries no stado-managed programs");
        return Ok(());
    }
    crate::cli::table::print(
        &["ARTIFACT", "COMMIT", "BUILDER", "AGE", "REACHABLE", "BYTES"],
        &rows,
    );
    if repository.is_none() {
        println!(
            "\n{target}: no local checkout was found, so reachability is unknown rather than \
             answered; run this from the stado source tree to resolve it"
        );
    }
    if drifted != usize::default() {
        println!(
            "{target}: {drifted} of {} artifacts have no producer reachable from origin/main",
            rows.len()
        );
    }
    let replaced = carried
        .iter()
        .filter(|item| item.describes == Some(false))
        .count();
    if replaced != usize::default() {
        // Louder than drift, because the manifest is not merely absent: it
        // answers the provenance question, and its answer is about bytes that
        // are gone. Every reader downstream inherits that wrong answer.
        println!(
            "{target}: {replaced} artifact(s) were replaced after their manifest was written, so \
             the commit shown for them describes bytes that are no longer on the host"
        );
    }
    if helpers != usize::default() {
        // Not drift, and not nothing. Helpers are delivered one at a time to
        // solve one incident and are never removed, so the population only
        // grows; naming the count is what makes an operator notice that a

        // directory of them accumulated while nobody decided to keep any.
        println!(
            "{target}: {helpers} installed helper script(s) alongside, which carry no release \
             and are not counted above"
        );
    }
    Ok(())
}
