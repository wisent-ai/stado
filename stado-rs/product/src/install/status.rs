use super::{recipe, services, transaction};
use crate::{catalog::text, common::Runtime, paths, signing, source, state::ProductState};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::collections::BTreeSet;

pub fn inspect(
    runtime: &Runtime,
    product: &Value,
    surface: &str,
    host: Option<&str>,
) -> Result<Value> {
    let id = text(product, "id")?;
    let saved = ProductState::load(runtime, id, surface)?;
    let mut report = if let Some(state) = &saved {
        serde_json::to_value(state)?
    } else {
        json!({"product": id, "surface": surface, "status": "absent", "installed_paths": [], "source_revision": null, "host": host})
    };
    let declared = recipe(product, surface)?;
    let mut origin = None;
    let mut directory = None;
    let mut source_error = None;
    if let Some(repository) = declared["repository"].as_str() {
        match source::checkout(runtime, repository) {
            Ok(root) => {
                match source::git(&root, &["rev-parse", "origin/main"]) {
                    Ok(revision) => origin = Some(revision),
                    Err(error) => source_error = Some(format!("{error:#}")),
                }
                directory = Some(root);
            }
            Err(error) => source_error = Some(format!("{error:#}")),
        }
    }
    report["origin_revision"] = json!(origin);
    report["source_directory"] = json!(directory);
    let mut detail = "no recorded installation".to_owned();
    let mut blocked = String::new();
    let mut ready = false;
    let mut signatures = Vec::new();
    if let Some(state) = &saved {
        let mut errors = Vec::new();
        let mut seen = BTreeSet::new();
        for path in &state.installed_paths {
            if !path.exists() {
                errors.push(format!("installed path is absent: {}", path.display()));
                continue;
            }
            let physical = path.canonicalize()?;
            if seen.insert(physical.clone()) {
                let signature = signing::inspect(&physical)?;
                if !signing::acceptable(&signature) {
                    errors.push(format!(
                        "{}: {}: {}",
                        path.display(),
                        signature["state"],
                        signature["error"]
                    ));
                }
                if signature["state"] != "not_native" && signature["state"] != "not_applicable" {
                    signatures.push(signature);
                }
            }
        }
        if let Err(error) = transaction::ownership::verify(state) {
            errors.push(format!("{error:#}"));
        }
        if let Some((fault, repairable)) = paths::fault(&state.installed_paths, runtime)? {
            if repairable {
                errors.push(fault);
            } else {
                blocked = fault;
            }
        }
        if state.status == "absent" {
            report["status"] = json!("absent");
        } else if state.status == "installing"
            || state.status == "removing"
            || state.status == "rolling_back"
        {
            report["status"] = json!("incomplete");
            detail = format!(
                "{} was interrupted; repeat that operation or roll back its recorded artifact",
                state.status
            );
        } else if !errors.is_empty() {
            report["status"] = json!("drifted");
            detail = errors.join("; ");
        } else if state.installed_paths.is_empty() {
            report["status"] = json!("unknown");
            detail = "receipt names no installed artifact".to_owned();
        } else if let Some(release) = &state.release {
            let accepted = release["coordinate"]["source_revision"]
                .as_str()
                .context("release receipt has no accepted source revision")?;
            if state.source_revision.as_deref() != Some(accepted) {
                report["status"] = json!("drifted");
                detail = "receipt source differs from its accepted immutable release".to_owned();
            } else {
                report["status"] = json!("ready");
                detail = "installed bytes match the accepted signed release".to_owned();
                ready = blocked.is_empty();
            }
        } else if let Some(error) = source_error {
            report["status"] = json!("unknown");
            detail = error;
        } else if state.source_revision.is_none() {
            report["status"] = json!("unknown");
            detail = "installed bytes have no verified source binding".to_owned();
        } else if state.source_revision != origin && directory.is_some() {
            report["status"] = json!("stale");
            detail = format!(
                "installed {}, origin/main {}",
                state.source_revision.as_deref().unwrap_or("unknown"),
                origin.as_deref().unwrap_or("unknown")
            );
        } else {
            report["status"] = json!("ready");
            detail =
                "installed files and stable code identities match their recorded source".to_owned();
            ready = blocked.is_empty();
        }
        if surface == "service" && state.status != "absent" {
            match host.or(state.host.as_deref()) {
                Some(host) => match services::observe(product, host) {
                    Ok(service) => {
                        ready &= service["ready"] == true;
                        report["service"] = service;
                    }
                    Err(error) => {
                        ready = false;
                        blocked = format!("managed service observation failed: {error:#}");
                    }
                },
                None => {
                    ready = false;
                    blocked = "service readiness requires its recorded --host".to_owned();
                }
            }
        }
    }
    if !ready && report["status"] == "ready" {
        report["status"] = json!("blocked");
    }
    report["signatures"] = json!(signatures);
    report["readiness"] = json!({"ready": ready, "detail": detail, "blocked": blocked});
    Ok(report)
}
