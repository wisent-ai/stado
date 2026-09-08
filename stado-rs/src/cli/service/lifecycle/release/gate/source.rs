//! Rolling one release back, and moving the directory's immutable source
//! with one that stuck.

use super::*;

/// Relink the previous release and bring the unit back.
///
/// Takes the release's own [`ServiceReleaseOptions`] rather than restating
/// three of its fields: the sole caller already holds it, and the file
/// carries the same shape for `secret-sync`, `file-sync` and `file-fetch`.
/// Eight loose parameters also put the release quality gate over
/// `clippy::too_many_arguments`, which is denied there, so no product release
/// could be submitted.
pub(super) async fn rollback_service_release(
    options: &ServiceReleaseOptions<'_>,
    previous: &str,
    target: &targets::ComputeTarget,
    declared: &ManagedService,
    sudo_password: Option<&str>,
    runner: &crate::deploy::Runner,
) -> Result<(), CmdError> {
    update(
        options.name,
        options.host,
        None,
        None,
        Some(previous),
        false,
        false,
    )
    .await?;
    let report = if options.reload_unit {
        service::reload_service_with_password(target, declared, sudo_password, runner).await
    } else {
        service::restart_service_with_password(target, declared, sudo_password, runner).await
    }
    .map_err(click)?;
    if report.succeeded("restarted") {
        Ok(())
    } else {
        Err(CmdError::click(format!(
            "rollback relinked {previous}, but restart failed: {}",
            report.failure()
        )))
    }
}

/// Move the service directory's immutable source with a successful product
/// release. Without this write the release runner advances `current` on the
/// host while `service converge` keeps the old artifact in the declaration and
/// can later put that old release back.
pub(super) async fn record_released_service_source(
    options: &ServiceReleaseOptions<'_>,
    artifact: &crate::release_control::ReleaseArtifactRef,
) -> Result<(), CmdError> {
    let source_ref = artifact
        .archive_uri
        .strip_suffix("/release.tar.gz")
        .ok_or_else(|| {
            CmdError::click(format!(
                "release archive URI {:?} has no service artifact coordinate",
                artifact.archive_uri
            ))
        })?;
    // The decision read: whether the pin has to move at all. A directory that
    // already names this artifact is not written, so a release that changed
    // nothing does not spend a compare-and-swap or advance the counter.
    let document = registry::fetch_document().await?;
    let logical = released_route(&document, options.name)?;
    if !source_pin_moves(&document, &logical, source_ref, &artifact.artifact_sha256)? {
        return Ok(());
    }
    // Pure: the artifact coordinate is already fixed by the release that just
    // succeeded, so pinning it is a function of the document it is pinned in.
    // `advance_generation` runs INSIDE the transform, on the document that
    // round read, because the counter it derives belongs to that document.
    registry::commit_document(|current| {
        let mut document = current.clone();
        let logical = released_route(&document, options.name)?;
        if !source_pin_moves(&document, &logical, source_ref, &artifact.artifact_sha256)? {
            // Another writer pinned the same artifact first. Its document is
            // already the answer, so this round republishes it verbatim rather
            // than advancing a counter for a change nobody made.
            return Ok(document);
        }
        let source = document
            .get_mut("service_directory")
            .and_then(|directory| directory.get_mut("services"))
            .and_then(|services| services.get_mut(&logical))
            .and_then(Value::as_object_mut)
            .and_then(|entry| entry.get_mut("declaration"))
            .and_then(Value::as_object_mut)
            .and_then(|declaration| declaration.get_mut("source"))
            .and_then(Value::as_object_mut)
            .ok_or_else(|| CmdError::click("release service route disappeared"))?;
        source.insert("artifact".to_string(), json!(source_ref));
        source.insert(
            "sha256".to_string(),
            json!(artifact.artifact_sha256.as_str()),
        );
        crate::service_resolution::advance_generation(&mut document).map_err(CmdError::click)?;
        Ok(document)
    })
    .await?;
    Ok(())
}

/// The one directory route that carries this managed service. Ambiguity is
/// refused rather than guessed: pinning the wrong route's artifact is how a
/// release lands on a service nobody released.
fn released_route(document: &Value, name: &str) -> Result<String, CmdError> {
    let services = document
        .get("service_directory")
        .and_then(|directory| directory.get("services"))
        .and_then(Value::as_object)
        .ok_or_else(|| CmdError::click("registry carries no service directory"))?;
    let matching = services
        .iter()
        .filter(|(logical, entry)| {
            logical.as_str() == name
                || entry.get("managed_service").and_then(Value::as_str) == Some(name)
                || placement_declares_unit(document, entry, logical, name)
        })
        .map(|(logical, _)| logical.clone())
        .collect::<Vec<_>>();
    match matching.as_slice() {
        [logical] => Ok(logical.clone()),
        [] => Err(CmdError::click(format!(
            "service directory carries no route for managed service {name:?}"
        ))),
        several => Err(CmdError::click(format!(
            "managed service {name:?} is shared by {} directory routes ({}); refusing to \
             change an ambiguous declaration",
            several.len(),
            several.join(", ")
        ))),
    }
}

/// Whether a placement-backed route's profile installs `unit` for this service.
///
/// A placement-backed route MUST leave `managed_service` absent - the schema
/// refuses it, because the unit is declared once per host inside the profile.
/// Reading only the absent field made every such service unreachable from a
/// unit name: on 2026-09-05 `service release com.wisent.always-on.brama` moved
/// `current` to the new digest and then failed with "carries no route", so the
/// host ran one release while the directory still described another.
fn placement_declares_unit(document: &Value, entry: &Value, logical: &str, unit: &str) -> bool {
    let Some(profile_name) = entry.get("placement_profile").and_then(Value::as_str) else {
        return false;
    };
    document
        .get("placement_profiles")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter(|profile| profile.get("name").and_then(Value::as_str) == Some(profile_name))
        .filter_map(|profile| profile.get("hosts").and_then(Value::as_object))
        .flat_map(|hosts| hosts.values())
        .filter_map(|host| host.get("units").and_then(Value::as_object))
        .filter_map(|units| units.get(logical))
        .filter_map(|declared| declared.get("unit").and_then(Value::as_str))
        .any(|declared| declared == unit)
}

/// Whether pinning `artifact`/`sha256` on this route would change anything.
///
/// A route that declares no deployable source has nothing to pin: its version
/// is delivered by the release plane (`release_control`) and the directory
/// carries only its address. That is not a failure of the release that just
/// landed, and treating it as one aborted the command after `current` had
/// already moved.
fn source_pin_moves(
    document: &Value,
    logical: &str,
    artifact: &str,
    sha256: &str,
) -> Result<bool, CmdError> {
    let Some(source) = document
        .get("service_directory")
        .and_then(|directory| directory.get("services"))
        .and_then(|services| services.get(logical))
        .and_then(|entry| entry.get("declaration"))
        .and_then(|declaration| declaration.get("source"))
        .and_then(Value::as_object)
    else {
        return Ok(false);
    };
    Ok(
        source.get("artifact").and_then(Value::as_str) != Some(artifact)
            || source.get("sha256").and_then(Value::as_str) != Some(sha256),
    )
}
