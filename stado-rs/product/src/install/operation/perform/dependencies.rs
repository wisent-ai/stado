//! The catalogue dependencies of one product installation, each installed,
//! met by an installation in progress, skipped for its alternative, or
//! refused.

use super::super::super::{plan::prerequisites::Enclosing, recipe};
use crate::catalog;
use crate::common::Runtime;
use anyhow::{bail, Context, Result};
use serde_json::{json, Value};
use std::path::Path;

/// The installation whose dependencies are installed: what `perform` was
/// asked for, with the choices it settled.
pub(super) struct Request<'a> {
    pub runtime: &'a Runtime,
    pub document: &'a Value,
    pub product: &'a Value,
    pub id: &'a str,
    pub surface: &'a str,
    pub host: Option<&'a str>,
    pub without: &'a [String],
    pub with_prerequisites: bool,
    pub lock: &'a Path,
}

/// Install every dependency the catalogue declares for `request`'s surface
/// and answer what became of each, in the receipt's `dependencies` form.
pub(super) fn install(request: Request<'_>, stack: &mut Vec<Enclosing>) -> Result<Vec<Value>> {
    let Request {
        runtime,
        document,
        product,
        id,
        surface,
        host,
        without,
        with_prerequisites,
        lock,
    } = request;
    let mut dependencies = Vec::new();
    let Some(declared) = product.get("dependencies") else {
        return Ok(dependencies);
    };
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
                match dependency["bound_because"].as_str() {
                    Some(because) => bail!(
                        "{id}/{surface} cannot be installed without {dependency_id}: \
                         the catalogue binds it by design: {because}"
                    ),
                    None => bail!(
                        "{id}/{surface} cannot be installed without {dependency_id}: \
                         the catalogue declares no alternative for it"
                    ),
                }
            };
            eprintln!(
                "{id}: installed without {dependency_id}/{dependency_surface}: {alternative}"
            );
            dependencies.push(json!({"product": dependency_id, "surface": dependency_surface, "checked": false, "without": true, "alternative": alternative}));
            continue;
        }
        // Products may depend on each other. A dependency that is
        // an installation already in progress above this one (Tama
        // depending back on the Oko being installed) is placed by
        // that installation once this one is done, so it is met,
        // not followed into a cycle.
        let dependency_node = format!("{dependency_id}/{dependency_surface}");
        if stack.iter().any(|frame| frame.node() == dependency_node) {
            let reason = "the installation in progress that this one is a dependency of places it";
            eprintln!("{id}: {dependency_node} is met: {reason}");
            dependencies.push(json!({"product": dependency_id, "surface": dependency_surface, "checked": false, "enclosing": true, "reason": reason}));
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
            continue;
        }
        // A product this host's update policy holds is installed, repaired
        // or rebuilt by no one but the operator, as a dependency least of
        // all: no dependent's installation runs its installation. One whose
        // receipt says installed is used as it is, whatever state its files
        // are in; one that is not installed (removed, or never installed
        // here) is not installed for it: a dependent with the catalogue's
        // alternative goes on without it, one bound to it is refused naming
        // the hold.
        let policy = crate::install::policy::Policy::load(&runtime.home)?;
        if let crate::install::policy::Verdict::Held(hold) =
            policy.verdict(dependency_id, chrono::Local::now().time())
        {
            let installed =
                crate::state::ProductState::load(runtime, dependency_id, dependency_surface)?
                    .is_some_and(|state| state.status.complete());
            if installed {
                eprintln!(
                    "{id}: uses {dependency_node} as it is installed, without checking or \
                     rebuilding it: {hold}"
                );
                dependencies.push(
                    json!({"product": dependency_id, "surface": dependency_surface,
                    "checked": false, "held": hold, "used_as_installed": true}),
                );
                continue;
            }
            match dependency["alternative"].as_str() {
                Some(alternative) => {
                    eprintln!(
                        "{id}: installed without {dependency_node}, which is not installed as a \
                         dependency: {hold}; {alternative}"
                    );
                    dependencies.push(json!({"product": dependency_id, "surface": dependency_surface,
                        "checked": false, "without": true, "held": hold, "alternative": alternative}));
                    continue;
                }
                None => bail!(
                    "{id}/{surface} needs {dependency_node}, and {hold}; the catalogue gives {id} \
                     no alternative to it, so {id} is not installed either. Release the hold, or \
                     install {id} where {dependency_id} may run"
                ),
            }
        }
        crate::common::mark_step(
            lock,
            &format!("dependency {dependency_id}/{dependency_surface}"),
        )?;
        // A service dependency runs beside its dependent, on the
        // host the installation names (this one, checked above).
        let dependency_host = host.filter(|_| dependency_surface == "service");
        let installed_dependency = super::perform(
            runtime,
            document,
            dependency_product,
            dependency_surface,
            dependency_host,
            None,
            without,
            with_prerequisites,
            stack,
        );
        // Every product runs without the others: a dependency the
        // catalogue gives an alternative for is one this product
        // works without, so its failed installation stops only
        // itself. One bound by design stops this installation.
        match (installed_dependency, dependency["alternative"].as_str()) {
            (Ok(dependency_state), _) => {
                dependencies.push(json!({"product": dependency_id, "surface": dependency_surface,
                    "checked": true, "install_decision": dependency_state.extra.get("install_decision"),
                    "dependencies": dependency_state.extra.get("dependencies")}));
            }
            (Err(failed), Some(alternative)) => {
                eprintln!(
                    "{id}: installed without {dependency_id}/{dependency_surface}, whose installation failed: {failed:#}; {alternative}"
                );
                dependencies.push(
                    json!({"product": dependency_id, "surface": dependency_surface,
                    "checked": true, "without": true, "failed": format!("{failed:#}"),
                    "alternative": alternative}),
                );
            }
            (Err(failed), None) => return Err(failed),
        }
    }
    Ok(dependencies)
}
