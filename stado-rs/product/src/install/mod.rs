mod after_install;
pub mod plan;
mod recipes;
mod services;
pub mod status;
mod sweep;
mod transaction;
use crate::{
    catalog,
    common::{emit, now, Arguments, Runtime},
    state::{self, ProductState},
};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

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
    // An exact release is a verified archive for this machine's platform, the
    // same bytes whichever surface places them. A service surface already runs
    // only on the host it names (above), so it takes the coordinate as the CLI
    // does: a product outside release control — the vault's own Skarbiec — is
    // otherwise installable only by a source build on its host, which needs a
    // signing identity that host's vault may not yet be able to tag.
    if pin.is_some()
        && (!matches!(surface, "cli" | "service") || selected["kind"] != "stado-release")
    {
        bail!("exact release coordinates require a stado-release recipe on the cli or service surface; no source build or host deployment was started");
    }
    if pin.is_some() && surface == "cli" && host.is_some() {
        bail!("exact release coordinates install a CLI on the machine that runs this command; --host names a service host. No source build or host deployment was started");
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
        let scratch = plan.scratch.clone();
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
            let outcomes = after_install::run(id, surface, host, steps, &installed)?;
            installed
                .extra
                .insert("after_install".to_owned(), json!(outcomes));
        }
        transaction::ownership::verify(&installed)?;
        installed.status = "installed".to_owned();
        installed.installed_at = now();
        installed.save(runtime)?;
        // The build run's trees were the placements' sources; installed, they
        // are read by nothing, and in a checkout they sit where the janitor
        // may not reach. A run that could not be shed is named in the state.
        if let Some(run) = &scratch {
            if let Err(error) = crate::common::runs::shed(run) {
                installed
                    .extra
                    .insert("scratch_kept".to_owned(), json!(format!("{error:#}")));
            }
        }
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
        "install" | "update" => {
            let mut building = runtime.clone();
            building.create_checkouts = true;
            serde_json::to_value(perform(
                &building,
                &document,
                product,
                surface,
                host,
                coordinate,
                &without,
                &mut Vec::new(),
            )?)?
        }
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
