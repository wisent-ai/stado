//! `stado service directory publish` — write this machine's forward markers
//! from the directory, and report every marker the directory does not declare.

use serde_json::{json, Value};

use crate::observations;

use crate::cli::registry;
use crate::cli::CmdError;

use crate::cli::directory::document::{directory, endpoint_url, this_target, DIRECTORY_KEY};
use crate::cli::directory::report::markers::{
    adapter_url, prune_outcome, sweep_markers, write_forward_marker,
};

/// Write `~/.stado/forwards/<service>.local` for every service the directory
/// gives this machine an address for, and report every marker it does not.
///
/// Owner-only, and written through a temporary file and a rename so a reader
/// never sees half an address. Skarbiec's reader refuses anything else - it
/// requires an owner-owned regular file, no group or world write, and exactly
/// one bounded URL - so writing it any other way produces a file the consumer
/// rejects.
///
/// This end of the directory had a writer and no remover, so the markers only
/// ever accumulated. operator-host carries 11 of them and the directory
/// declares 3: one endpoint under three names (`stado-api.local`,
/// `stado-object.local`, `stado-object-api.local`, all `http://127.0.0.1:8765`),
/// one relationship under two ports (`weles-skarbiec.local` -> 8786 and
/// `skarbiec-weles.local` -> 6119, while the registry says 19095), and
/// `stado-weles-api.local` -> 8766, a port nothing has ever bound. None of
/// those is a stale copy of a live answer: each is an address some consumer on
/// this host will dial forever, and no run of anything has ever contradicted
/// one. So every run now states the difference between what the directory
/// declares and what the directory left behind.
///
/// Reporting is the default and deletion is not. `--prune` removes exactly the
/// undeclared markers and prints each one; a marker the directory does declare
/// is never touched by either path.
pub(in crate::cli::directory) async fn publish(
    service: Option<String>,
    target: Option<String>,
    prune: bool,
    as_json: bool,
) -> Result<(), CmdError> {
    // A fossil is a marker no declaration claims, so it belongs to no service
    // and cannot be named by one. Pruning under `--service` would either
    // delete nothing -- the named service is declared, or the run has already
    // failed below -- or quietly sweep ten markers the operator did not
    // mention. Refusing is the only reading of the two flags that is not a
    // surprise.
    if prune && service.is_some() {
        return Err(CmdError::usage(
            "--prune compares the whole directory against every marker present and cannot be \
             scoped with --service; publish one service, then run `publish --prune` to sweep",
        ));
    }
    let document = registry::fetch_document().await?;
    let target = match target {
        Some(value) => value,
        None => this_target().await?,
    };
    let markers = declared_markers(&document, &target).map_err(|detail| {
        CmdError::click(detail).stating(crate::primitives::failure::FailureCode::Config)
    })?;
    let forwards = forwards_dir().map_err(|detail| {
        CmdError::click(detail).stating(crate::primitives::failure::FailureCode::Config)
    })?;
    std::fs::create_dir_all(&forwards)?;
    let mut published: Vec<Value> = Vec::new();
    let mut skipped: Vec<Value> = Vec::new();
    for marker in &markers.addressed {
        if service
            .as_deref()
            .is_some_and(|wanted| wanted != marker.service)
        {
            continue;
        }
        let path = forwards.join(format!("{}.local", marker.service));
        write_forward_marker(&path, &marker.url)?;
        published.push(json!({
            "service": marker.service,
            "url": marker.url,
            "source": marker.source,
            "marker": path.display().to_string(),
        }));
    }
    for (name, reason) in &markers.skipped {
        if service.as_deref().is_some_and(|wanted| wanted != name) {
            continue;
        }
        skipped.push(json!({ "service": name, "reason": reason }));
    }
    if service.is_some() && published.is_empty() && skipped.is_empty() {
        return Err(CmdError::click(format!(
            "{DIRECTORY_KEY} declares no service named {}",
            service.unwrap_or_default()
        ))
        .stating(crate::primitives::failure::FailureCode::NotFound));
    }
    // The whole declared set, never the filtered one. `--service` narrows what
    // this run writes; it cannot narrow what the directory says, and a sweep
    // that mistook "not published just now" for "not declared" would report
    // every live marker on the host as a fossil. A service this host reaches
    // through its own adapter is declared for it too: the marker is the
    // address it dials, so sweeping it would delete a live answer.
    let declared: std::collections::BTreeSet<&str> = markers
        .addressed
        .iter()
        .map(|marker| marker.service.as_str())
        .collect();
    let sweep = sweep_markers(&forwards, &declared)?;
    let mut pruned: Vec<Value> = Vec::new();
    let mut failed = 0;
    if prune {
        // One unlink that will not go through does not end the sweep. The
        // markers already removed are removed, the rest are still findings,
        // and an operator who is shown neither has to start the whole
        // comparison again to learn how far it got.
        for fossil in &sweep.fossil {
            let (status, detail) = match std::fs::remove_file(&fossil.marker) {
                Ok(()) => ("removed", Value::Null),
                Err(error) => {
                    failed += 1;
                    ("failed", Value::String(error.to_string()))
                }
            };
            pruned.push(json!({
                "service": fossil.service,
                "url": fossil.value,
                "marker": fossil.marker.display().to_string(),
                "status": status,
                "error": detail,
            }));
        }
    }
    // Of the markers PRESENT, how many a declaration accounts for. Not the
    // size of the declared set: the measured sentence is "11 markers, of which
    // the directory declares 3", and a declared service whose marker this run
    // did not write is not one of the eleven.
    let accounted = sweep.present - sweep.fossil.len();
    let fossils: Vec<Value> = sweep
        .fossil
        .iter()
        .map(|fossil| {
            json!({
                "service": fossil.service,
                "url": fossil.value,
                "marker": fossil.marker.display().to_string(),
                "age_seconds": fossil.age_seconds,
            })
        })
        .collect();
    if as_json {
        // `fossil` is the finding and is emitted whether or not anything was
        // removed, so a run with `--prune` and one without report the same
        // population and differ only in what they did about it.
        let mut report = json!({
            "target": target,
            "published": published,
            "skipped": skipped,
            "fossil": fossils,
            "markers_present": sweep.present,
            "markers_declared": accounted,
        });
        if prune {
            report["pruned"] = Value::Array(pruned);
        }
        println!("{}", serde_json::to_string_pretty(&report)?);
        return prune_outcome(failed);
    }
    // Each marker is an address a consumer on this host will dial without ever
    // asking again, so the line that announces writing one also says when
    // anyone last confirmed it answers.
    let seen = observations::load();
    for entry in &published {
        let service = entry.get("service").and_then(Value::as_str).unwrap_or("");
        println!(
            "{service} -> {} ({}) [observed {}]",
            entry.get("url").and_then(Value::as_str).unwrap_or(""),
            entry.get("marker").and_then(Value::as_str).unwrap_or(""),
            observations::describe_in(&seen, &observations::service_fact(service, &target))
        );
    }
    for entry in &skipped {
        println!(
            "{}: {}",
            entry.get("service").and_then(Value::as_str).unwrap_or(""),
            entry.get("reason").and_then(Value::as_str).unwrap_or("")
        );
    }
    // The value and the age are both printed because both are needed to
    // decide: the value says which of three names for one endpoint this is,
    // and the age says whether anyone could still plausibly be holding it.
    for fossil in &sweep.fossil {
        println!(
            "fossil {} -> {} ({}, last written {}); {DIRECTORY_KEY} declares no endpoint \
             for {target}",
            fossil.service,
            fossil.value,
            fossil.marker.display(),
            fossil.age()
        );
    }
    for entry in &pruned {
        println!(
            "pruned {}: {} {}{}",
            entry.get("service").and_then(Value::as_str).unwrap_or(""),
            entry.get("status").and_then(Value::as_str).unwrap_or(""),
            entry.get("marker").and_then(Value::as_str).unwrap_or(""),
            entry
                .get("error")
                .and_then(Value::as_str)
                .map_or_else(String::new, |error| format!(": {error}"))
        );
    }
    // The count is the thing an operator notices. Eleven markers where three
    // are declared is not eight small mistakes; it is a directory that
    // accumulated while nobody decided to keep any of it.
    if !sweep.fossil.is_empty() {
        println!(
            "{} marker(s) in {}, {accounted} declared by the directory for {target}, {} it \
             does not",
            sweep.present,
            forwards.display(),
            sweep.fossil.len()
        );
        if !prune {
            println!("nothing was removed; `--prune` removes exactly the markers listed above");
        }
    }
    prune_outcome(failed)
}

/// One marker the directory gives this machine an address for.
pub(crate) struct DeclaredMarker {
    pub(crate) service: String,
    pub(crate) url: String,
    pub(crate) source: &'static str,
}

/// Every marker the directory declares for one machine, and every service it
/// names that this machine gets no address for, with the reason.
pub(crate) struct DeclaredMarkers {
    pub(crate) addressed: Vec<DeclaredMarker>,
    pub(crate) skipped: Vec<(String, String)>,
}

/// What the directory in `document` says `target` dials for each service:
/// the directory's own endpoint for that machine, otherwise the bind of the
/// one resolver adapter it declares for the service.
///
/// The operator's `publish` and the resolver's keeper both answer from this,
/// so the markers a host carries cannot depend on which of the two wrote last.
pub(crate) fn declared_markers(document: &Value, target: &str) -> Result<DeclaredMarkers, String> {
    let block = directory(document).map_err(|error| error.to_string())?;
    let services = block
        .get("services")
        .and_then(Value::as_object)
        .ok_or_else(|| format!("{DIRECTORY_KEY}.services: must be an object"))?;
    // The target's own declaration, for the services the directory places
    // elsewhere. Read once: the adapter set does not change inside one run.
    let target_entry = document
        .get("targets")
        .and_then(Value::as_array)
        .and_then(|targets| {
            targets
                .iter()
                .find(|candidate| candidate.get("name").and_then(Value::as_str) == Some(target))
        })
        .cloned()
        .unwrap_or_else(|| json!({}));
    let mut markers = DeclaredMarkers {
        addressed: Vec::new(),
        skipped: Vec::new(),
    };
    for (name, entry) in services {
        if let Some(url) = endpoint_url(entry, target) {
            markers.addressed.push(DeclaredMarker {
                service: name.clone(),
                url: url.to_string(),
                source: "directory-endpoint",
            });
            continue;
        }
        match adapter_url(&target_entry, name) {
            Ok(Some(url)) => markers.addressed.push(DeclaredMarker {
                service: name.clone(),
                url,
                source: "resolver-adapter",
            }),
            Ok(None) => markers.skipped.push((
                name.clone(),
                format!(
                    "{DIRECTORY_KEY} declares no endpoint for {target} and its resolver declares \
                     no {name} adapter"
                ),
            )),
            Err(reason) => markers.skipped.push((name.clone(), reason)),
        }
    }
    Ok(markers)
}

/// `~/.stado/forwards`, where every marker lives.
fn forwards_dir() -> Result<std::path::PathBuf, String> {
    let home = std::env::var("HOME").map_err(|_| "HOME is not set".to_string())?;
    Ok(std::path::Path::new(&home).join(".stado").join("forwards"))
}

/// Write every marker the directory declares for `target` whose file is
/// missing or holds another address, and name each one written.
///
/// The resolver calls this whenever it loads the directory. Markers were
/// written only by an operator running `stado service directory publish` on
/// that machine, so a host where nobody had was missing them for good: the
/// vault host had no `brama.local` while the directory declared Brama on it,
/// and Weles, which reads that file at startup, exited on every launch. A
/// marker already holding the declared address is left alone, so a refresh
/// that changes nothing writes nothing; undeclared markers stay a finding for
/// `publish`, never a deletion here.
pub(crate) fn keep_declared_markers(document: &Value, target: &str) -> Result<Vec<String>, String> {
    let markers = declared_markers(document, target)?;
    let forwards = forwards_dir()?;
    std::fs::create_dir_all(&forwards)
        .map_err(|error| format!("cannot create {}: {error}", forwards.display()))?;
    let mut written = Vec::new();
    for marker in &markers.addressed {
        let path = forwards.join(format!("{}.local", marker.service));
        let current = std::fs::read_to_string(&path).unwrap_or_default();
        if current.trim() == marker.url {
            continue;
        }
        write_forward_marker(&path, &marker.url)
            .map_err(|error| format!("cannot write {}: {error}", path.display()))?;
        written.push(format!("{} -> {}", marker.service, marker.url));
    }
    Ok(written)
}
