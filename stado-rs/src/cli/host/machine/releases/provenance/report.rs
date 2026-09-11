use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::machine::releases::provenance::{
    CarriedArtifact, DeliveryReceipt, READ_PROVENANCE_BODY,
};

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
    let mut records: std::collections::BTreeMap<String, crate::binary::provenance::Provenance> =
        std::collections::BTreeMap::new();
    let mut unreadable: Vec<String> = Vec::new();
    let mut helpers: usize = 0;
    let mut present: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut markers: usize = 0;
    let mut receipts: Vec<DeliveryReceipt> = Vec::new();
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
            if kind == "marker" {
                markers += 1;
                continue;
            }
            if !digest.is_empty() && digest != "-" {
                present.insert(name.to_string(), digest);
            }
            names.insert(name.to_string());
        } else if let Some(document) = line.strip_prefix("STADO-MANIFEST ") {
            match serde_json::from_str::<crate::binary::provenance::Provenance>(document.trim()) {
                Ok(record) => {
                    names.insert(record.artifact.clone());
                    records.insert(record.artifact.clone(), record);
                }
                // A manifest that cannot be parsed is not a missing manifest:
                // something wrote a file there and it says nothing usable.
                Err(error) => unreadable.push(error.to_string()),
            }
        } else if let Some(document) = line.strip_prefix("STADO-RECEIPT ") {
            match serde_json::from_str::<DeliveryReceipt>(document.trim()) {
                // A receipt names an artefact the delivery path installed, so
                // it is a population member as much as a record: a binary
                // delivered and then deleted is a row worth seeing.
                Ok(receipt) => {
                    names.insert(receipt.binary.clone());
                    receipts.push(receipt);
                }
                Err(error) => unreadable.push(error.to_string()),
            }
        }
    }

    let repository = crate::binary::provenance::local_repo();
    let now = chrono::Utc::now();
    let carried: Vec<CarriedArtifact> = names
        .into_iter()
        .map(|artifact| {
            let record = records.remove(&artifact);
            let installed = present.get(&artifact);
            // The receipt that describes the bytes in place, newest first when
            // a host kept several. Matched on the digest, never on the name
            // alone: a receipt for a version this file is not is a record of a
            // different delivery, and answering with it would be the confident
            // wrong answer this command exists to prevent.
            let receipt = record.is_none().then_some(()).and_then(|()| {
                let mut candidates: Vec<&DeliveryReceipt> = receipts
                    .iter()
                    .filter(|receipt| receipt.binary == artifact)
                    .filter(|receipt| {
                        installed
                            .map(|actual| receipt.artifact_sha256.eq_ignore_ascii_case(actual))
                            .unwrap_or_default()
                    })
                    .collect();
                candidates.sort_by(|left, right| right.installed_at.cmp(&left.installed_at));
                candidates.first().map(|receipt| (*receipt).clone())
            });
            let commit = match (&record, &receipt) {
                (Some(record), _) => Some(record.commit.clone()),
                (None, Some(receipt)) => Some(receipt.source_commit.clone()),
                (None, None) => None,
            };
            let reachable = match (&commit, &repository) {
                (None, _) => Some(false),
                (Some(commit), _) if !crate::binary::provenance::is_commit_id(commit) => {
                    Some(false)
                }
                (Some(_), None) => None,
                (Some(commit), Some(repository)) => Some(
                    crate::binary::provenance::reachable_in_repo(commit, repository),
                ),
            };
            let stamp = match (&record, &receipt) {
                (Some(record), _) => Some(record.at.clone()),
                (None, Some(receipt)) => Some(receipt.installed_at.clone()),
                (None, None) => None,
            };
            let age_seconds = stamp.as_deref().and_then(|stamp| {
                chrono::DateTime::parse_from_rfc3339(stamp)
                    .ok()
                    .map(|stamp| (now - stamp.with_timezone(&chrono::Utc)).num_seconds())
            });
            let describes = match (&record, &receipt, installed) {
                (Some(record), _, Some(actual)) => Some(record.sha256.eq_ignore_ascii_case(actual)),
                // Digest-matched above, so a receipt-accounted row describes
                // its bytes by construction.
                (None, Some(_), Some(_)) => Some(true),
                _ => None,
            };
            CarriedArtifact {
                artifact,
                record,
                receipt,
                reachable,
                describes,
                age_seconds,
            }
        })
        .collect();

    // Drift is an answer, not a missing one. `reachable == None` means no
    // checkout here could resolve the commit, and counting it as drift made
    // the trailer say "have no producer reachable from origin/main" about
    // artifacts nobody had been able to ask about - the exact fold the
    // `reachable` field exists to prevent.
    let counts = super::trailers::Counts {
        drifted: carried
            .iter()
            .filter(|item| item.reachable == Some(false))
            .count(),
        unresolved: carried
            .iter()
            .filter(|item| item.reachable.is_none())
            .count(),
        helpers,
        markers,
    };
    let drifted = counts.drifted;

    if json {
        let artifacts: Vec<Value> = carried
            .iter()
            .map(|item| {
                json!({
                    "artifact": item.artifact,
                    "accounted_by": item.accounted_by().as_str(),
                    "manifest": item.record.is_some(),
                    "receipt": item.receipt.is_some(),
                    "version": item.version(),
                    "commit": item.commit(),
                    "sha256": item
                        .record
                        .as_ref()
                        .map(|record| record.sha256.clone())
                        .or_else(|| {
                            item.receipt
                                .as_ref()
                                .map(|receipt| receipt.artifact_sha256.clone())
                        }),
                    // The archive the delivery verified on the way in, for the
                    // rows a receipt accounts for. Kept beside the artefact
                    // digest rather than folded into it: one names the bytes
                    // in place, the other names what they were unpacked from.
                    "archive_sha256": item.receipt.as_ref().map(|receipt| receipt.sha256.clone()),
                    "platform": item.receipt.as_ref().map(|receipt| receipt.platform.clone()),
                    "builder": item.accounted().then(|| item.builder()),
                    "at": item.stamp(),
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
                "unresolved": counts.unresolved,
                "helper_scripts": counts.helpers,
                "delivery_markers": counts.markers,
            }))?
        );
        return Ok(());
    }

    let rows: Vec<Vec<String>> = carried
        .iter()
        .map(|item| {
            let age = match (item.age_seconds, item.accounted()) {
                (Some(seconds), _) => {
                    crate::cli::registry::human_age(chrono::TimeDelta::seconds(seconds))
                }
                // A record whose timestamp will not parse was hand-edited; say
                // so instead of showing an age.
                (None, true) => "unknown".to_string(),
                (None, false) => "never".to_string(),
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
                item.accounted_by().as_str().to_string(),
                item.commit(),
                item.builder(),
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
    crate::cli::reporting::table::print(
        &[
            "ARTIFACT",
            "ACCOUNTED",
            "COMMIT",
            "BUILDER",
            "AGE",
            "REACHABLE",
            "BYTES",
        ],
        &rows,
    );
    super::trailers::print(target, &carried, &counts, rows.len(), repository.is_none());
    Ok(())
}
