use serde_json::{json, Value};

use crate::cli::CmdError;

use crate::cli::host::machine::releases::provenance::{
    CarriedArtifact, DeliveryReceipt, READ_PROVENANCE_BODY,
};

/// `stado release provenance --host TARGET [--json]` — what TARGET carries,
/// and who produced it.
///
/// The command that was missing while the only record of what was running
/// the control plane was a version string the repository had never heard
/// of. Every artifact under the host's Stado bin directory gets a row,
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
    let resolved = crate::cli::canonical_host(target).await?;
    let runner = crate::deploy::production_runner();
    let script = format!("set -euo pipefail\n{READ_PROVENANCE_BODY}");
    let output = crate::deploy::host_channel::run_script(&resolved, &script, &runner)
        .await
        .map_err(CmdError::from)?;
    if !output.succeeded() {
        return Err(CmdError::click(format!(
            "{target}: cannot read provenance manifests: {}",
            crate::deploy::host_channel::last_error_line(&output, "remote provenance read failed")
        ))
        .stating(crate::primitives::failure::FailureCode::InfraDown));
    }

    let mut names: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
    let mut records: std::collections::BTreeMap<String, crate::binary::provenance::Provenance> =
        std::collections::BTreeMap::new();
    let mut unreadable: Vec<String> = Vec::new();
    let mut helpers: Vec<String> = Vec::new();
    let mut present: std::collections::BTreeMap<String, String> = std::collections::BTreeMap::new();
    let mut markers: Vec<String> = Vec::new();
    // Why a row's reachability is unresolved when a check was attempted and
    // could not answer, said beside the rows rather than dropped.
    let mut unresolved_causes: Vec<String> = Vec::new();
    let mut receipts: Vec<DeliveryReceipt> = Vec::new();
    for line in output.stdout.lines() {
        if let Some(artifact) = line.strip_prefix("STADO-ARTIFACT ") {
            // `<kind> <digest> <name>`. A helper script has no release behind
            // it, so listing it beside the control-plane binary answers a
            // question nobody asked and hides the one that matters.
            // A line without all three words names no artefact this report
            // can place; skipped, the artefact vanished from the report.
            let words = artifact.trim().split_once(' ').and_then(|(kind, rest)| {
                rest.split_once(' ')
                    .map(|(digest, name)| (kind, digest.trim(), name.trim()))
            });
            let Some((kind, digest, name)) = words.filter(|(_kind, _digest, name)| !name.is_empty())
            else {
                unreadable.push(format!(
                    "artifact line names no kind, digest and name: {artifact}"
                ));
                continue;
            };
            let digest = digest.to_string();
            if kind == "script" {
                helpers.push(name.to_string());
                continue;
            }
            if kind == "marker" {
                markers.push(name.to_string());
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

    // No checkout here leaves reachability unresolved for every row; a binary
    // or directory that cannot be read, or a git that cannot run, is said
    // instead of being read as "no checkout".
    let repository = match crate::binary::provenance::local_repo() {
        Ok(repository) => repository,
        Err(cause) => {
            unresolved_causes.push(cause);
            None
        }
    };
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
                        installed.is_some_and(|actual| receipt.artifact_sha256.eq_ignore_ascii_case(actual))
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
                (Some(commit), Some(repository)) => {
                    match crate::binary::provenance::reachable_in_repo(commit, repository) {
                        Ok(reachable) => Some(reachable),
                        Err(cause) => {
                            unresolved_causes.push(format!("{artifact}: {cause}"));
                            None
                        }
                    }
                }
            };
            let stamp = match (&record, &receipt) {
                (Some(record), _) => Some(record.at.clone()),
                (None, Some(receipt)) => Some(receipt.installed_at.clone()),
                (None, None) => None,
            };
            // A stamp that will not parse leaves the age unknown; the table
            // says "unreadable stamp" for it.
            let age_seconds = match stamp.as_deref().map(chrono::DateTime::parse_from_rfc3339) {
                Some(Ok(stamp)) => Some((now - stamp.with_timezone(&chrono::Utc)).num_seconds()),
                Some(Err(_)) | None => None,
            };
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
        helpers: helpers.len(),
        markers: markers.len(),
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
                "unresolved_causes": unresolved_causes,
                "drifted": drifted,
                "unresolved": counts.unresolved,
                "helper_scripts": counts.helpers,
                "delivery_markers": counts.markers,
            }))?
        );
        return Ok(());
    }

    super::text::print(&super::text::Findings {
        target,
        carried: &carried,
        counts: &counts,
        unreadable: &unreadable,
        unresolved_causes: &unresolved_causes,
        repository_missing: repository.is_none(),
    });
    Ok(())
}
