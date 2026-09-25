use super::{perform, recipe, status};
use crate::{
    catalog,
    common::{capture, emit, lock, now, Arguments, Runtime},
    source,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{collections::BTreeMap, fs, io::Write, process::Command};

pub fn run(arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let args = Arguments::from_matches(arguments);
    if !args.positional.is_empty() {
        bail!("sync does not take positional arguments");
    }
    let surface = args.required("--surface")?;
    if !matches!(surface, "cli" | "desktop" | "service") {
        bail!("surface must be cli, desktop or service");
    }
    let host = args.optional("--host")?;
    if surface == "service" && host.is_none() {
        bail!("service reconciliation requires --host");
    }
    let path = runtime.home.join(".stado/products/sync.lock");
    let mut writer = if args.has("--dry-run") {
        None
    } else {
        match lock(&path) {
            Ok(writer) => Some(writer),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::WouldBlock) =>
            {
                emit(
                    &json!({"state": "busy", "holder": fs::read_to_string(&path).unwrap_or_default()}),
                )?;
                return Ok(0);
            }
            Err(error) => return Err(error),
        }
    };
    if let Some(writer) = &mut writer {
        writer.set_len(0)?;
        writer.write_all(
            serde_json::to_string(
                &json!({"pid": std::process::id(), "surface": surface, "started_at": now()}),
            )?
            .as_bytes(),
        )?;
        writer.sync_all()?;
    }
    let document = catalog::current(runtime)?;
    let mut rows = Vec::new();
    let mut fetched = BTreeMap::new();
    for product in document["products"]
        .as_array()
        .context("catalog has no products")?
    {
        if recipe(product, surface).is_err() {
            continue;
        }
        let mut row = json!({"product": product["id"], "surface": surface, "decision": "skipped", "status": "unknown",
            "source_revision": null, "origin_revision": null, "note": "", "detail": ""});
        let outcome = (|| -> Result<()> {
            let selected = recipe(product, surface)?;
            let root = match selected["repository"].as_str() {
                Some(repository) => match source::checkout(runtime, repository) {
                    Ok(root) => Some(root),
                    Err(error) => {
                        row["decision"] = json!("held");
                        row["status"] = json!(if error.is::<source::MissingCheckout>() {
                            "no-checkout"
                        } else {
                            "unknown"
                        });
                        row["detail"] = json!(format!("{error:#}"));
                        return Ok(());
                    }
                },
                None => None,
            };
            if let Some(root) = &root {
                if args.has("--fetch") && !args.has("--dry-run") {
                    let fetched = fetched.entry(root.clone()).or_insert_with(|| {
                        let output = capture(
                            Command::new("git")
                                .args(["fetch", "--quiet", "origin"])
                                .env("GIT_TERMINAL_PROMPT", "0")
                                .current_dir(root),
                        )
                        .map_err(|error| format!("{error:#}"))?;
                        if output.status.success() {
                            Ok(())
                        } else {
                            Err(format!(
                                "git fetch origin failed in {} ({}): {}{}",
                                root.display(),
                                output.status,
                                String::from_utf8_lossy(&output.stdout),
                                String::from_utf8_lossy(&output.stderr)
                            ))
                        }
                    });
                    if let Err(error) = fetched {
                        row["decision"] = json!("held");
                        row["detail"] = json!(error);
                        return Ok(());
                    }
                }
            }
            let observed = status::inspect(runtime, product, surface, host)?;
            copy_status(&mut row, &observed);
            if observed["readiness"]["ready"] == true {
                return Ok(());
            }
            if let Some(reason) = observed["readiness"]["blocked"]
                .as_str()
                .filter(|s| !s.is_empty())
            {
                row["decision"] = json!("held");
                row["detail"] = json!(reason);
                return Ok(());
            }
            if let Some(root) = &root {
                let modified =
                    source::git(root, &["status", "--porcelain", "--untracked-files=no"])?;
                if !modified.is_empty() {
                    row["decision"] = json!("held");
                    row["detail"] = json!(format!("canonical main has tracked edits: {modified}"));
                    return Ok(());
                }
                let ancestor = capture(
                    Command::new("git")
                        .args(["merge-base", "--is-ancestor", "HEAD", "origin/main"])
                        .current_dir(root),
                )?;
                if !ancestor.status.success() {
                    row["decision"] = json!("held");
                    row["detail"] = json!("canonical main cannot fast-forward to origin/main");
                    return Ok(());
                }
            }
            if args.has("--dry-run") {
                row["decision"] = json!("would-install");
                return Ok(());
            }
            perform(
                runtime,
                &document,
                product,
                surface,
                host,
                None,
                &mut Vec::new(),
            )?;
            let observed = status::inspect(runtime, product, surface, host)?;
            copy_status(&mut row, &observed);
            row["decision"] = json!(if observed["readiness"]["ready"] == true {
                "installed"
            } else {
                "failed"
            });
            Ok(())
        })();
        if let Err(error) = outcome {
            row["decision"] = json!("failed");
            row["detail"] = json!(format!("{error:#}"));
        }
        rows.push(row);
    }
    let failed = rows.iter().any(|row| row["decision"] == "failed");
    emit(&json!(rows))?;
    Ok(i32::from(failed))
}

fn copy_status(row: &mut Value, observed: &Value) {
    row["status"] = observed["status"].clone();
    row["source_revision"] = observed["source_revision"].clone();
    row["origin_revision"] = observed["origin_revision"].clone();
    row["detail"] = observed["readiness"]["detail"].clone();
}
