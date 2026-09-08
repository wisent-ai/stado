//! The other side of `record.rs`: stopping a web product's unit and taking
//! both registry halves back out.

use serde_json::{json, Value};

use crate::cli::web::unit_label;
use crate::cli::CmdError;
use crate::config::WebApiProduct;
use crate::deploy::{host_channel, production_runner, service};

use super::click;

/// Stop one web product's unit and take it out of management.
///
/// A unit that is not there is `unchanged` rather than an error, because this
/// is the half of `stado web remove` that has to be able to run twice: an
/// operator whose first removal failed at the DNS record must be able to run
/// the whole command again, and a declaration whose unit was never deployed
/// still has to be removable.
///
/// Both registry halves go in one update. `service declare` writes the
/// directory entry and the target's managed record together, and the
/// validator correctly refuses a directory entry pointing at no managed
/// service — so dropping only one of them makes the document unwritable by
/// anything else. The publication counter advances with the directory change
/// for the same reason it advances on the way in.
pub(crate) async fn retire(name: &str, declared: &WebApiProduct) -> Result<Value, CmdError> {
    // Neither kind of hostname-only product owns a unit, so there is nothing
    // on any host to stop and no host name in the declaration to reach for.
    // `stado web remove` still retracts the hostname from the edge; that is
    // the caller's half. For an upstream service this is the load-bearing
    // case: stopping Brama because someone removed a hostname in front of it
    // would be this command reaching into a service it does not own.
    if !declared.owns_a_unit() {
        let detail = match (declared.redirect_to(), declared.upstream_service()) {
            (Some(target), _) => format!("{name} is a redirect to {target}, which runs nothing"),
            (None, Some(service)) => format!(
                "{name} is a hostname in front of the registry service {service:?}, which keeps running and is not touched here"
            ),
            (None, None) => unreachable!("a product with no unit is one of the two kinds"),
        };
        return Ok(json!({
            "unit": Value::Null,
            "state": "no-unit",
            "detail": detail,
        }));
    }
    let host = declared.host();
    let label = unit_label(name);
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    // The target's own declaration, read locally. `declared_matching` raises
    // for an empty result, and "the unit is not there" is the answer this
    // function is required to give rather than an error to raise.
    let found = service::declared_services(&target)
        .into_iter()
        .find(|candidate| candidate.matches(name) || candidate.matches(&label));

    let Some(found) = found else {
        return Ok(json!({
            "unit": label,
            "host": host,
            "change": "unchanged",
            "detail": format!("{host} does not manage {label}"),
        }));
    };

    // A placeholder record — written by a declaration that no deploy has
    // followed — names no unit and no file, so asking launchd to boot out an
    // empty label would fail on a state that is registry-only by design.
    if !(found.unit_id().is_empty() && found.path.is_empty()) {
        let runner = production_runner();
        let sudo_password = if crate::deploy::service::UnitDomain::from_path(&found.path)
            .requires_privileged_bootstrap()
        {
            crate::cli::service::host_sudo_password(&target).await?
        } else {
            None
        };
        let report = service::retire_service(&target, &found, sudo_password.as_deref(), &runner)
            .await
            .map_err(click)?;
        if !report.succeeded("retired") {
            // Forgetting a unit that is still serving is the state this whole
            // command family exists to prevent, so the declaration stays until
            // the host confirms the unit is stopped.
            return Err(CmdError::click(format!(
                "{host}: could not stop {}: {}; it is still declared in the registry",
                found.unit_id(),
                report.failure()
            )));
        }
    }

    // Expected generation: this read, taken after the unit was stopped. The
    // stop is not repeatable, so a lost race is reported rather than retried —
    // re-applying the removal against a newer document would erase whatever
    // the winning writer said about this service while the host was draining.
    let (mut document, expected_generation) =
        crate::cli::registry::fetch_versioned_document().await?;
    let removed = service::remove_service(&mut document, host, found.unit_id()).map_err(click)?;
    let directory_removed = document
        .get_mut("service_directory")
        .and_then(Value::as_object_mut)
        .and_then(|directory| directory.get_mut("services"))
        .and_then(Value::as_object_mut)
        .is_some_and(|services| services.remove(name).is_some());
    if directory_removed {
        // A directory that cannot carry a counter is a document this command
        // did not write and must not silently repair; the removal still stands.
        let _ = crate::service_resolution::advance_generation(&mut document);
    }
    let generation =
        crate::cli::registry::push_document_if(&document, &expected_generation).await?;

    Ok(json!({
        "unit": removed.unit_id(),
        "host": host,
        "change": "removed",
        "directory_entry": if directory_removed { "removed" } else { "absent" },
        "registry_generation": generation,
    }))
}
