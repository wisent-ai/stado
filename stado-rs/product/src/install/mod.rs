pub mod plan;
mod recipes;
mod services;
pub mod status;
mod sweep;
mod transaction;
use crate::{
    catalog,
    common::{checked, emit, now, Arguments, Runtime},
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

/// What an installation is bound to beyond the catalog's recipe: an exact
/// qualified release, or an exact canonical commit a source build exports.
#[derive(Clone, Copy)]
pub enum Coordinate<'a> {
    Release { version: &'a str, revision: &'a str },
    SourceCommit(&'a str),
}

/// `without` names products this machine does without. A dependency on one of
/// them is skipped only when the catalogue gives it an `alternative`; any
/// other is refused, so nothing is installed half-wired.
// Every argument is a distinct coordinate of one installation: the runtime,
// the catalogue, the product, its surface, the host, an exact coordinate, the
// dependencies left out and the recursion stack; a struct would move the
// same eight names one level out.
#[allow(clippy::too_many_arguments)]
pub fn perform(
    runtime: &Runtime,
    document: &Value,
    product: &Value,
    surface: &str,
    host: Option<&str>,
    coordinate: Option<Coordinate<'_>>,
    without: &[String],
    stack: &mut Vec<String>,
) -> Result<ProductState> {
    let id = catalog::text(product, "id")?;
    let selected = recipe(product, surface)?;
    if surface == "service" && host.is_none() {
        bail!("service installation requires --host");
    }
    // A service surface installs its files on this machine and then ensures
    // the unit on `--host`. For another host that restarted the unit there on
    // the files it already had, and left the new build on this machine.
    if let Some(other) = host.filter(|host| surface == "service" && !runtime.is_this_host(host)) {
        bail!(
            "a service installation puts its files on the machine that runs this command, so \
             --host {other} would restart {other}'s unit on the files it already has; run the \
             installation on {other}. Nothing was built, installed or restarted"
        );
    }
    let (pin, source_commit) = match coordinate {
        Some(Coordinate::Release { version, revision }) => (Some((version, revision)), None),
        Some(Coordinate::SourceCommit(commit)) => (None, Some(commit)),
        None => (None, None),
    };
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
        let _writer =
            runtime.surface_lock(&state::path(runtime, id, surface)?.with_extension("lock"))?;
        let existing = ProductState::load(runtime, id, surface)?;
        let plan = plan::planning::select(
            &plan::planning::Request {
                runtime,
                product,
                selected,
                surface,
                host,
                pin,
                source_commit,
                id,
            },
            existing.as_ref(),
        )?;
        // An explicit --without replaces the choice; otherwise the choice the
        // installation being replaced recorded holds, so `update` and `sync`
        // do not install what the user chose to do without.
        let without: Vec<String> = if without.is_empty() {
            existing
                .as_ref()
                .and_then(|state| state.extra.get("without"))
                .and_then(Value::as_array)
                .map(|names| {
                    names
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect()
                })
                .unwrap_or_default()
        } else {
            without.to_vec()
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
                if without.iter().any(|name| name == dependency_id) {
                    let Some(alternative) = dependency["alternative"].as_str() else {
                        bail!(
                            "{id}/{surface} cannot be installed without {dependency_id}: \
                             the catalogue declares no alternative for it"
                        );
                    };
                    eprintln!(
                        "{id}: installed without {dependency_id}/{dependency_surface}: {alternative}"
                    );
                    dependencies.push(json!({"product": dependency_id, "surface": dependency_surface, "checked": false, "without": true, "alternative": alternative}));
                    continue;
                }
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
                        None,
                        None,
                        &without,
                        stack,
                    )?;
                    dependencies.push(json!({"product": dependency_id, "surface": dependency_surface, "checked": true}));
                }
            }
        }
        let mut installed = transaction::commit(runtime, id, surface, host, selected, plan)?;
        if !without.is_empty() {
            installed.extra.insert("without".to_owned(), json!(without));
        }
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
            // executing the binary this install replaced. Without it an
            // install leaves the object API running the replaced image, and
            // anything that checks the resident identity fails on exactly that.
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
            let mut outcomes = Vec::new();
            for step in steps.as_array().context("after_install must be an array")? {
                let words = step
                    .as_array()
                    .context("after_install command must be argv")?
                    .iter()
                    .map(|value| {
                        value
                            .as_str()
                            .context("after_install arguments must be strings")
                    })
                    .collect::<Result<Vec<_>>>()?;
                // A step that hands the release archive to a reconciler has
                // nothing to hand when the installation was built from source.
                // An option whose value is a release placeholder is then left
                // out, so the reconciler still runs on what the source install
                // did place (Stado's recycles the units executing the binary
                // it replaced); a placeholder in any other position cannot be
                // dropped, and that step is recorded as not run, with the
                // reason, over a working install.
                let is_placeholder =
                    |word: &str| matches!(word, "{release_archive}" | "{release_archive_sha256}");
                let words: Vec<&str> = if archive.is_none() {
                    let mut kept = Vec::with_capacity(words.len());
                    let mut index = 0;
                    while index < words.len() {
                        let is_option = words[index].starts_with("--");
                        if is_option
                            && words
                                .get(index + 1)
                                .is_some_and(|value| is_placeholder(value))
                        {
                            index += 2;
                            continue;
                        }
                        kept.push(words[index]);
                        index += 1;
                    }
                    kept
                } else {
                    words
                };
                if archive.is_none() && words.iter().any(|word| is_placeholder(word)) {
                    let reason =
                        "this installation was built from source, so there is no verified \
                         release archive to hand to the step; readers it would reconcile \
                         keep their image until a release install";
                    eprintln!("{id}: after_install step {words:?} not run: {reason}");
                    outcomes.push(json!({"argv": words, "ran": false, "reason": reason}));
                    continue;
                }
                let argv = words
                    .iter()
                    .map(|word| match *word {
                        "{release_archive}" => archive.clone().context(
                            "after_install names {release_archive}, and this installation \
                             came from no verified release archive",
                        ),
                        "{release_archive_sha256}" => archive_sha256.clone().context(
                            "after_install names {release_archive_sha256}, and this \
                             installation came from no verified release archive",
                        ),
                        word => Ok(word.to_owned()),
                    })
                    .collect::<Result<Vec<_>>>()?;
                let (name, arguments) = argv
                    .split_first()
                    .context("after_install has an empty command")?;
                let binary = installed.installed_paths.iter().find(|path| path.file_name().and_then(|s| s.to_str()) == Some(name.as_str()))
                    .with_context(|| format!("after_install names a binary this installation did not produce: {name}"))?;
                checked(Command::new(binary).args(arguments))?;
                outcomes.push(json!({"argv": argv, "ran": true}));
            }
            installed
                .extra
                .insert("after_install".to_owned(), json!(outcomes));
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
    let mut runtime = runtime.clone();
    runtime.wait_for_writer = args.has("--wait");
    let runtime = &runtime;
    if args.positional.len() != 1 {
        bail!("{action} requires exactly one product");
    }
    let surface = args.required("--surface")?;
    if !matches!(surface, "cli" | "desktop" | "service") {
        bail!("surface must be cli, desktop or service");
    }
    let host = args.optional("--host")?;
    let coordinate = match (
        args.optional("--release-version")?,
        args.optional("--source-commit")?,
    ) {
        (None, None) => None,
        (Some(_), _) | (_, Some(_)) if action != "install" && action != "update" => {
            bail!("release coordinates apply only to install and update")
        }
        (Some(version), Some(revision)) => Some(Coordinate::Release { version, revision }),
        (None, Some(commit)) => Some(Coordinate::SourceCommit(commit)),
        (Some(_), None) => bail!("--release-version needs the --source-commit bound to it"),
    };
    let document = catalog::current(runtime)?;
    let product = catalog::product(&document, &args.positional[0])?;
    let without: Vec<String> = args
        .optional("--without")?
        .map(|list| {
            list.split(',')
                .map(str::trim)
                .filter(|name| !name.is_empty())
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    for name in &without {
        catalog::product(&document, name)?;
    }
    if args.has("--check-arguments") {
        emit(&serde_json::json!({
            "action": action,
            "product": args.positional[0],
            "surface": surface,
            "arguments": "accepted",
        }))?;
        return Ok(0);
    }
    let report = match action {
        "status" => status::inspect(runtime, product, surface, host)?,
        "install" | "update" => serde_json::to_value(perform(
            runtime,
            &document,
            product,
            surface,
            host,
            coordinate,
            &without,
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
