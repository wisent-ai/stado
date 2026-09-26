pub mod plan;
mod recipes;
mod release;
mod services;
pub mod status;
mod sweep;
mod transaction;
use crate::{
    catalog,
    common::{checked, emit, lock, now, Arguments, Runtime},
    source,
    state::{self, ProductState},
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::process::Command;

pub fn recipe<'a>(product: &'a Value, surface: &str) -> Result<&'a Value> {
    product["installations"]
        .as_array()
        .and_then(|rows| rows.iter().find(|row| row["surface"] == surface))
        .with_context(|| format!("{} has no installation recipe for {surface}", product["id"]))
}

pub fn perform(
    runtime: &Runtime,
    document: &Value,
    product: &Value,
    surface: &str,
    host: Option<&str>,
    pin: Option<(&str, &str)>,
    stack: &mut Vec<String>,
) -> Result<ProductState> {
    let id = catalog::text(product, "id")?;
    let selected = recipe(product, surface)?;
    if surface == "service" && host.is_none() {
        bail!("service installation requires --host");
    }
    if pin.is_some() && (surface != "cli" || selected["kind"] != "stado-release" || host.is_some())
    {
        bail!("exact release coordinates require a local CLI stado-release recipe; no source build or host deployment was started");
    }
    let node = format!("{id}/{surface}");
    if stack.contains(&node) {
        bail!("product dependency cycle: {} -> {node}", stack.join(" -> "));
    }
    stack.push(node);
    let result = (|| {
        let _writer = lock(&state::path(runtime, id, surface)?.with_extension("lock"))?;
        let existing = ProductState::load(runtime, id, surface)?;
        let plan = if let Some(incomplete) = existing
            .as_ref()
            .filter(|state| state.status == "installing")
        {
            if incomplete.recipe != *selected || incomplete.host.as_deref() != host {
                bail!("unfinished installation is bound to another recipe or host; roll it back first");
            }
            if let Some((version, revision)) = pin {
                let accepted = incomplete
                    .release
                    .as_ref()
                    .context("unfinished installation was not an exact release")?;
                if accepted["coordinate"]["version"] != version
                    || accepted["coordinate"]["source_revision"] != revision
                {
                    bail!("unfinished installation is bound to another release coordinate; roll it back first");
                }
            }
            serde_json::from_value(
                incomplete
                    .extra
                    .get("prepared")
                    .cloned()
                    .context("interrupted installation has no retained artifact plan")?,
            )?
        } else {
            if existing
                .as_ref()
                .is_some_and(|state| state.status == "removing" || state.status == "rolling_back")
            {
                bail!("finish the recorded removal or rollback before installing");
            }
            if let Some((version, revision)) = pin {
                release::prepare(product, version, revision, runtime)?
            } else {
                let repository = selected["repository"]
                    .as_str()
                    .or(product["repository"].as_str())
                    .context("installation has no source repository")?;
                let root = source::checkout(runtime, repository)?;
                if source::git(&root, &["status", "--porcelain", "--untracked-files=no"])?
                    .is_empty()
                {
                    let ancestor = crate::common::capture(
                        Command::new("git")
                            .args(["merge-base", "--is-ancestor", "HEAD", "origin/main"])
                            .current_dir(&root),
                    )?;
                    if ancestor.status.success() {
                        source::advance(&root, false)?;
                    }
                }
                recipes::prepare(runtime, product, selected, surface, &root)?
            }
        };
        let mut dependencies = Vec::new();
        if let Some(declared) = product.get("dependencies") {
            for dependency in declared
                .as_array()
                .context("product dependencies must be an array")?
            {
                if dependency["for_surface"]
                    .as_str()
                    .is_some_and(|s| s != surface)
                {
                    continue;
                }
                let dependency_id = catalog::text(dependency, "product")?;
                let dependency_surface = catalog::text(dependency, "surface")?;
                let dependency_product = catalog::product(document, dependency_id)?;
                let blocked = if dependency_surface == "service" && host.is_none() {
                    Some("no --host was given".to_owned())
                } else {
                    recipe(dependency_product, dependency_surface)
                        .err()
                        .map(|error| error.to_string())
                };
                if let Some(reason) = blocked {
                    eprintln!("{id}: not checking {dependency_id}/{dependency_surface}: {reason}");
                    dependencies.push(json!({"product": dependency_id, "surface": dependency_surface, "checked": false, "reason": reason}));
                } else {
                    perform(
                        runtime,
                        document,
                        dependency_product,
                        dependency_surface,
                        host,
                        None,
                        stack,
                    )?;
                    dependencies.push(json!({"product": dependency_id, "surface": dependency_surface, "checked": true}));
                }
            }
        }
        let mut installed = transaction::commit(runtime, id, surface, host, selected, plan)?;
        installed
            .extra
            .insert("dependencies".to_owned(), json!(dependencies));
        installed.save(runtime)?;
        if surface == "service" {
            let service = services::ensure(product, selected, host.unwrap())?;
            installed.extra.insert("service".to_owned(), service);
            installed.save(runtime)?;
        }
        if let Some(steps) = selected.get("after_install") {
            // `{release_archive}` and `{release_archive_sha256}` name the
            // verified archive this installation came from, so a step can hand
            // the exact bytes to the product's own reconciler: Stado's
            // `release converge-local-readers` restarts every unit still
            // executing the binary this install replaced. Without it the 0.22.5
            // install on 2026-09-26 left the object API on lukasz-macbook on
            // the replaced image, and the next build's resident-identity test
            // failed on exactly that.
            let archive = installed
                .release
                .as_ref()
                .and_then(|release| release["destination"].as_str())
                .map(str::to_owned);
            let archive_sha256 = installed
                .release
                .as_ref()
                .and_then(|release| release["artifact"]["artifact_sha256"].as_str())
                .map(str::to_owned);
            for step in steps.as_array().context("after_install must be an array")? {
                let argv = step
                    .as_array()
                    .context("after_install command must be argv")?
                    .iter()
                    .map(|value| {
                        let word = value
                            .as_str()
                            .context("after_install arguments must be strings")?;
                        match word {
                            "{release_archive}" => archive.clone().context(
                                "after_install names {release_archive}, and this installation \
                                 came from no verified release archive",
                            ),
                            "{release_archive_sha256}" => archive_sha256.clone().context(
                                "after_install names {release_archive_sha256}, and this \
                                 installation came from no verified release archive",
                            ),
                            word => Ok(word.to_owned()),
                        }
                    })
                    .collect::<Result<Vec<_>>>()?;
                let (name, arguments) = argv
                    .split_first()
                    .context("after_install has an empty command")?;
                let binary = installed.installed_paths.iter().find(|path| path.file_name().and_then(|s| s.to_str()) == Some(name.as_str()))
                    .with_context(|| format!("after_install names a binary this installation did not produce: {name}"))?;
                checked(Command::new(binary).args(arguments))?;
            }
        }
        transaction::ownership::verify(&installed)?;
        installed.status = "installed".to_owned();
        installed.installed_at = now();
        installed.save(runtime)?;
        let observed = status::inspect(runtime, product, surface, host)?;
        installed
            .extra
            .insert("readiness".to_owned(), observed["readiness"].clone());
        installed.save(runtime)?;
        Ok(installed)
    })();
    stack.pop();
    result
}

pub fn run(action: &str, arguments: clap::ArgMatches, runtime: &Runtime) -> Result<i32> {
    if action == "sync" {
        return sweep::run(arguments, runtime);
    }
    let args = Arguments::from_matches(arguments);
    if args.positional.len() != 1 {
        bail!("{action} requires exactly one product");
    }
    let surface = args.required("--surface")?;
    if !matches!(surface, "cli" | "desktop" | "service") {
        bail!("surface must be cli, desktop or service");
    }
    let host = args.optional("--host")?;
    let pin = match (
        args.optional("--release-version")?,
        args.optional("--source-commit")?,
    ) {
        (None, None) => None,
        (Some(version), Some(revision)) if action == "install" || action == "update" => {
            Some((version, revision))
        }
        (Some(_), Some(_)) => bail!("release coordinates apply only to install and update"),
        _ => bail!("--release-version and --source-commit must be supplied together"),
    };
    let document = catalog::current(runtime)?;
    let product = catalog::product(&document, &args.positional[0])?;
    let report = match action {
        "status" => status::inspect(runtime, product, surface, host)?,
        "install" | "update" => serde_json::to_value(perform(
            runtime,
            &document,
            product,
            surface,
            host,
            pin,
            &mut Vec::new(),
        )?)?,
        "remove" => serde_json::to_value(transaction::lifecycle::remove(
            runtime, product, surface, host,
        )?)?,
        "rollback" => {
            serde_json::to_value(transaction::lifecycle::rollback(runtime, product, surface)?)?
        }
        _ => bail!("unknown installation operation {action}"),
    };
    emit(&report)?;
    Ok(0)
}
