//! One product installation from an exact source or release coordinate.

mod dependencies;

use super::super::{
    handoff, operation, plan, plan::prerequisites::Enclosing, recipe, services, settle, status,
    transaction, Coordinate,
};
use crate::catalog;
use crate::common::{now, Runtime};
use crate::state::{self, ProductState};
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};

/// `without` names products this machine does without. A dependency on one of
/// them is skipped only when the catalogue gives it an `alternative`; any
/// other is refused, so nothing is installed half-wired. `stack` holds the
/// installations in progress this one is a dependency of, with their revisions.
// The caller names runtime, catalog, product, destination, source coordinate,
// retained choices, prerequisite policy and recursion stack explicitly; this
// operation does not need a second parameter object.
#[allow(clippy::too_many_arguments)]
pub fn perform(
    runtime: &Runtime,
    document: &Value,
    product: &Value,
    surface: &str,
    host: Option<&str>,
    coordinate: Option<Coordinate<'_>>,
    without: &[String],
    with_prerequisites: bool,
    stack: &mut Vec<Enclosing>,
) -> Result<ProductState> {
    let id = catalog::text(product, "id")?;
    let selected = recipe(product, surface)?;
    if surface == "service" && host.is_none() {
        bail!("service installation requires --host");
    }
    // A service surface installs its files on this machine and then ensures
    // the unit on `--host`. For another host that restarted the unit there on
    // the files it already had, and left the new build on this machine.
    if let Some(other) = host.filter(|_| surface == "service") {
        if !runtime.is_this_host(other)? {
            bail!(
                "a service installation puts its files on the machine that runs this command, so \
                 --host {other} would restart {other}'s unit on the files it already has; run the \
                 installation on {other}. Nothing was built, installed or restarted"
            );
        }
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
    if pin.is_some() && (surface == "desktop" || selected["kind"] != "stado-release") {
        bail!("exact release coordinates require a stado-release recipe on the cli or service surface; no source build or host deployment was started");
    }
    if pin.is_some() && surface == "cli" && host.is_some() {
        bail!("exact release coordinates install a CLI on the machine that runs this command; --host names a service host. No source build or host deployment was started");
    }
    let node = format!("{id}/{surface}");
    if stack.iter().any(|frame| frame.node() == node) {
        let path: Vec<String> = stack.iter().map(Enclosing::node).collect();
        bail!("product dependency cycle: {} -> {node}", path.join(" -> "));
    }
    let is_dependency = !stack.is_empty();
    let enclosing = stack.clone();
    let revision = plan::prerequisites::revision(runtime, product, surface, coordinate)?;
    stack.push(Enclosing::new(id, surface, &revision));
    let result = (|| {
        let required = plan::prerequisites::requirements(
            runtime, document, product, surface, &revision, &enclosing, without,
        )?;
        let lock = state::path(runtime, id, surface)?.with_extension("lock");
        let writer = operation::lock_surface(runtime, (id, surface), &lock, selected)?;
        // The revision this installation delivers is held by it in the
        // wait-for graph while it runs, so a wait elsewhere that needs this
        // revision (a host whose repair it carries) and is needed by this
        // installation is refused with the cycle named, not left standing.
        let delivered = stado_wait::Resource::new(
            stado_wait::ResourceKind::ProductRevision,
            format!("{id}@{revision}"),
        )
        .map_err(|cause| anyhow::anyhow!("{id}/{surface}: the revision it delivers cannot be named in the wait-for graph: {cause}"))?;
        let _delivering = stado_wait::hold(&delivered).map_err(|error| {
            anyhow::anyhow!("{id}/{surface}: recording that it delivers {id}@{revision} in the wait-for graph failed: {error}")
        })?;
        let existing = ProductState::load(runtime, id, surface)?;
        let request = plan::planning::Request {
            runtime,
            product,
            selected,
            surface,
            host,
            pin,
            source_commit,
            id,
        };
        let decision = plan::planning::decision(&request, existing.as_ref(), is_dependency)?;
        eprintln!(
            "{id}/{surface}: {}: {}",
            decision["action"], decision["reason"]
        );
        // An explicit --without replaces the choice; otherwise the choice the
        // installation being replaced recorded holds, so `update` and `sync`
        // do not install what the user chose to do without.
        let without: Vec<String> = if without.is_empty() {
            match existing
                .as_ref()
                .and_then(|state| state.extra.get("without"))
                .and_then(Value::as_array)
            {
                Some(names) => names
                    .iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect(),
                None => Vec::new(),
            }
        } else {
            without.to_vec()
        };
        super::prerequisites::install(
            super::prerequisites::Request {
                runtime,
                document,
                id,
                surface,
                without: &without,
                with_prerequisites,
                lock: &lock,
            },
            required,
            stack,
        )?;
        let dependencies = dependencies::install(
            dependencies::Request {
                runtime,
                document,
                product,
                id,
                surface,
                host,
                without: &without,
                with_prerequisites,
                lock: &lock,
            },
            stack,
        )?;
        if decision["action"] == "reuse" {
            let mut installed = existing.context("reuse requires an installed receipt")?;
            installed
                .extra
                .insert("install_decision".to_owned(), decision);
            installed
                .extra
                .insert("dependencies".to_owned(), json!(dependencies));
            installed.extra.insert("without".to_owned(), json!(without));
            installed.save(runtime)?;
            return Ok(installed);
        }
        let mut installing = runtime.clone();
        if pin.is_none() && cfg!(target_os = "macos") {
            installing.signing = runtime.signing.for_recipe(selected, &runtime.home)?;
        }
        let request = plan::planning::Request {
            runtime: &installing,
            product,
            selected,
            surface,
            host,
            pin,
            source_commit,
            id,
        };
        crate::common::mark_step(&lock, "build")?;
        let plan = plan::planning::select(&request, existing.as_ref())?;
        let scratch = plan.scratch.clone();
        let checkout = plan.source_directory.clone();
        let kept = plan.cache.clone();
        let resident_host = if surface == "cli" {
            services::migrate_host_unit(product, &runtime.home, &plan)?
        } else {
            None
        };
        // From here the unit carries the candidate's command line: an
        // installation that stops before committing renders it again through
        // the executable still installed, which the unit goes on starting.
        let installed_program = match resident_host {
            Some(_) => Some(services::installed_binary(&plan, id)?),
            None => None,
        };
        let admitted = match resident_host.as_deref() {
            Some(resident) => services::admit_units(&plan, id, resident),
            None => Ok(()),
        };
        let committed = admitted
            .and_then(|()| transaction::commit(&installing, id, surface, host, selected, plan));
        let mut installed = match (committed, installed_program) {
            (Ok(installed), _) => installed,
            (Err(failed), Some(program)) => {
                return Err(services::restored_after(
                    product,
                    &runtime.home,
                    &program,
                    failed,
                ))
            }
            (Err(failed), None) => return Err(failed),
        };
        installed
            .extra
            .insert("install_decision".to_owned(), decision);
        if let Some(path) = scratch
            .as_ref()
            .map(|run| run.join("cargo-cache.json"))
            .filter(|path| path.is_file())
        {
            installed.extra.insert(
                "cargo_cache".to_owned(),
                serde_json::from_slice(&std::fs::read(path)?)?,
            );
        }
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
        // The recipe, including after_install, comes from the committed catalog;
        // the receipt names that read.
        if let Some(read) = &runtime.catalog_read {
            installed
                .extra
                .insert("catalog".to_owned(), serde_json::to_value(read)?);
        }
        transaction::ownership::verify(&installed)?;
        installed.status = crate::state::InstallStatus::Installed;
        installed.installed_at = now();
        installed.save(runtime)?;
        let _writer = services::Activation {
            runtime,
            product,
            id,
            surface,
            host,
            resident_host: resident_host.as_deref(),
            recipe: selected,
            lock: &lock,
        }
        .finish(&mut installed, writer)?;
        settle::settle(
            &mut installed,
            scratch.as_deref(),
            checkout.as_deref(),
            kept.as_deref(),
        );
        let observed = status::inspect(runtime, product, surface, host)?;
        installed
            .extra
            .insert("readiness".to_owned(), observed["readiness"].clone());
        installed.save(runtime)?;
        Ok(installed)
    })();
    stack.pop();
    handoff::finished(result, runtime, id, surface, host, stack)
}
