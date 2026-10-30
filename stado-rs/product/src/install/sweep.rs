use super::{perform, policy, recipe, status, transaction::lifecycle::rollback};
use crate::{
    catalog::{self, text},
    common::{capture, emit, lock, Arguments, Runtime},
    source,
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    fs,
    path::{Path, PathBuf},
    process::Command,
};

pub fn run(arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    let args = Arguments::from_matches(arguments);
    if !args.positional.is_empty() {
        bail!("sync does not take positional arguments");
    }
    // clap's `--surface` parser (cli::lifecycle::surface) already refused any
    // surface it does not declare.
    let surface = args.required("--surface")?;
    let host = args.optional("--host")?;
    if surface == "service" && host.is_none() {
        bail!("service reconciliation requires --host");
    }
    let path = runtime.home.join(".stado/products/sync.lock");
    // The lock's own holder record (pid, birth, command, phase) names this
    // pass; nothing else is written into the file. A second record written
    // after it would land past the end of a shorter first one and leave zero
    // bytes in front of it, which no reader can parse.
    let _held = if args.has("--dry-run") {
        None
    } else {
        match lock(&path) {
            Ok(writer) => Some(writer),
            Err(error)
                if error
                    .downcast_ref::<std::io::Error>()
                    .is_some_and(|e| e.kind() == std::io::ErrorKind::WouldBlock) =>
            {
                // A holder record that cannot be read is said with its cause,
                // not printed as an empty holder.
                let holder = match fs::read_to_string(&path) {
                    Ok(text) => json!(text),
                    Err(error) => json!({ "unreadable": error.to_string() }),
                };
                emit(&json!({"state": "busy", "holder": holder}))?;
                return Ok(0);
            }
            Err(error) => return Err(error),
        }
    };
    let policy = policy::Policy::load(&runtime.home)?;
    // Sync installs what it reconciles, so it executes the committed catalog
    // (catalog::committed: the checkout's committed head), not its working
    // tree, and every installation it makes names that revision.
    let (document, read) = catalog::committed::for_installation(runtime, "", None)?;
    let mut runtime = runtime.clone();
    runtime.catalog_read = Some(read);
    let runtime = &runtime;
    // `--clone-missing`: a workspace Stado owns starts empty, and each product's
    // source is cloned into it the first time the product is reconciled.
    let cloning = args.has("--clone-missing") && !args.has("--dry-run");
    if cloning {
        fs::create_dir_all(&runtime.workspace)
            .with_context(|| format!("creating workspace {}", runtime.workspace.display()))?;
    }
    let mut sourcing = runtime.clone();
    sourcing.create_checkouts = cloning;
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
            "source_revision": null, "origin_revision": null, "note": "", "detail": "",
            "policy": policy.describe(text(product, "id")?)});
        let outcome = (|| -> Result<()> {
            let selected = recipe(product, surface)?;
            let root = match selected["repository"].as_str() {
                Some(repository) => match source::required_checkout(&sourcing, repository) {
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
            match policy.verdict(text(product, "id")?, chrono::Local::now().time()) {
                policy::Verdict::Install => {}
                policy::Verdict::Held(reason) => {
                    row["decision"] = json!("held");
                    row["detail"] = json!(reason);
                    return Ok(());
                }
                policy::Verdict::Deferred(reason) => {
                    row["decision"] = json!("deferred");
                    row["detail"] = json!(reason);
                    return Ok(());
                }
            }
            if args.has("--dry-run") {
                row["decision"] = json!("would-install");
                return Ok(());
            }
            let id = text(product, "id")?;
            let refusal = refused_path(runtime, id, surface);
            if let Some(refused) = refused_revision(&refusal)? {
                if observed["origin_revision"].as_str() == Some(refused.as_str()) {
                    row["decision"] = json!("held");
                    row["detail"] = json!(format!(
                        "origin/main is still {refused}, which product sync installed and rolled \
                         back because it failed its readiness check ({}); the next commit on \
                         origin/main is installed, or install this one by hand with `stado \
                         product update {id} --surface {surface}`",
                        refusal.display()
                    ));
                    return Ok(());
                }
            }
            let before = crate::state::ProductState::load(runtime, id, surface)?;
            perform(
                runtime,
                &document,
                product,
                surface,
                host,
                None,
                &[],
                false,
                &mut Vec::new(),
            )?;
            let observed = status::inspect(runtime, product, surface, host)?;
            copy_status(&mut row, &observed);
            let failure = if observed["readiness"]["ready"] == true {
                health(runtime, id, surface, recipe(product, surface)?)?
            } else {
                // `blocked` names what stops a ready installation; `detail`
                // says what the installed files are.
                Some(
                    [
                        &observed["readiness"]["blocked"],
                        &observed["readiness"]["detail"],
                    ]
                    .into_iter()
                    .filter_map(Value::as_str)
                    .filter(|text| !text.is_empty())
                    .collect::<Vec<_>>()
                    .join("; "),
                )
            };
            let Some(failure) = failure else {
                row["decision"] = json!("installed");
                remove_refusal(&refusal)?;
                return Ok(());
            };
            // The installation placed files that do not pass their readiness
            // check: a host that updates by itself puts back what ran before,
            // or nobody would notice until the product was next used.
            let installed = match observed["source_revision"].as_str() {
                Some(revision) => revision.to_owned(),
                None => "an unrecorded revision".to_owned(),
            };
            let refused = format!(
                "the installed {id} {installed} did not pass its readiness check: {failure}"
            );
            if observed["source_revision"].is_string() {
                crate::common::atomic_json(
                    &refusal,
                    &json!({"revision": installed, "reason": refused, "at": crate::common::now()}),
                )
                .with_context(|| {
                    format!(
                        "recording that {installed} was refused failed, so the next pass would \
                         install it again; make {} writable",
                        refusal.display()
                    )
                })?;
            }
            if before.is_none() {
                row["decision"] = json!("failed");
                row["detail"] = json!(format!(
                    "{refused}; no earlier installation exists to roll back to, so fix the \
                     product at origin/main or remove it with `stado product remove {id} \
                     --surface {surface}`"
                ));
                return Ok(());
            }
            match rollback(runtime, product, surface) {
                Ok(restored) => {
                    row["decision"] = json!("rolled-back");
                    row["source_revision"] = json!(restored.source_revision);
                    row["detail"] = json!(format!(
                        "{refused}; the earlier installation of {} was restored, and product \
                         sync installs {id} again once origin/main moves past {installed}",
                        match restored.source_revision.as_deref() {
                            Some(revision) => revision,
                            None => "an unrecorded revision",
                        }
                    ));
                }
                Err(error) => {
                    row["decision"] = json!("failed");
                    row["detail"] = json!(format!(
                        "{refused}; rolling it back failed too ({error:#}), so {id} stays on the \
                         revision that failed its check; run `stado product rollback {id} \
                         --surface {surface}` once the cause is fixed"
                    ));
                }
            }
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

/// Where product sync records the revision of `id`'s `surface` it installed,
/// found failing and rolled back, so the next pass does not install it again.
/// It is not a `.json` file: every `.json` file in a product's directory is
/// read as one of its surface receipts.
fn refused_path(runtime: &Runtime, id: &str, surface: &str) -> PathBuf {
    runtime
        .home
        .join(".stado/products")
        .join(id)
        .join(format!("{surface}.refused"))
}

fn refused_revision(path: &Path) -> Result<Option<String>> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(error).with_context(|| {
                format!(
                    "reading {} failed, so product sync cannot tell whether origin/main is a \
                     revision it already rolled back; make the file readable or remove it",
                    path.display()
                )
            })
        }
    };
    let value: Value = serde_json::from_str(&text).with_context(|| {
        format!(
            "{} is not JSON, so product sync cannot tell which revision it rolled back; \
             remove it and the next pass installs origin/main again",
            path.display()
        )
    })?;
    match value["revision"].as_str() {
        Some(revision) => Ok(Some(revision.to_owned())),
        None => bail!(
            "{} names no revision, so product sync cannot tell which revision it rolled back; \
             remove it and the next pass installs origin/main again",
            path.display()
        ),
    }
}

fn remove_refusal(path: &Path) -> Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| {
            format!(
                "removing {}, the record of a revision rolled back earlier, failed after a newer \
                 one installed; remove it by hand",
                path.display()
            )
        }),
    }
}

/// Run the recipe's `health` command against what was just installed: one of
/// the installation's own binaries, or Stado. `None` when it passes or the
/// recipe declares none; otherwise what it answered.
fn health(runtime: &Runtime, id: &str, surface: &str, recipe: &Value) -> Result<Option<String>> {
    let Some(words) = recipe.get("health").and_then(Value::as_array) else {
        return Ok(None);
    };
    let words = words
        .iter()
        .map(|word| word.as_str().context("health words are validated as text"))
        .collect::<Result<Vec<_>>>()?;
    let (name, arguments) = words
        .split_first()
        .context("health is validated as a non-empty argv")?;
    let installed = crate::state::ProductState::load(runtime, id, surface)?
        .with_context(|| format!("{id} {surface} has no receipt right after it was installed"))?;
    let produced = installed
        .installed_paths
        .iter()
        .find(|path| path.file_name().and_then(|s| s.to_str()) == Some(*name));
    let mut command = match produced {
        Some(binary) => Command::new(binary),
        None if *name == "stado" => crate::common::stado(),
        None => {
            return Ok(Some(format!(
                "its health command names {name}, which this installation did not place; name \
                 one of its binaries or stado in the catalog recipe"
            )))
        }
    };
    let output = capture(command.args(arguments))?;
    if output.status.success() {
        return Ok(None);
    }
    Ok(Some(format!(
        "its health command `{}` exited with {}: {}{}",
        words.join(" "),
        output.status,
        String::from_utf8_lossy(&output.stdout).trim(),
        String::from_utf8_lossy(&output.stderr).trim()
    )))
}
