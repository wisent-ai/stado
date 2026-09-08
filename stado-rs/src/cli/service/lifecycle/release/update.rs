//! `service update`: move an already-managed service onto a new artifact
//! version, an unpublished bundle, or back onto a previous one.

use super::*;

pub(crate) async fn update(
    name: &str,
    host: &str,
    reference: Option<&str>,
    archive: Option<&str>,
    rollback_to: Option<&str>,
    refresh_image: bool,
    json: bool,
) -> Result<(), CmdError> {
    let target = host_channel::canonical_target(host).await.map_err(click)?;
    // The service must already be managed here: this moves a unit forward, and
    // silently installing a version for a unit nobody runs would look like a
    // deployment while changing nothing.
    let services = declared_matching(name, Some(host)).await?;
    if services.is_empty() {
        return Err(CmdError::click(format!(
            "{host} does not manage {name}; deploy it first"
        )));
    }
    // The registry name is the unit label; the artifact directory is whatever
    // the unit's own program path reads from. Deriving it from the unit means
    // the new version lands where the running one is actually read, instead of
    // beside it under a directory that only matches the name.
    let runner = production_runner();
    let declared = &services[usize::default()];
    // Archive membership follows the actual executable vector in the unit.
    // `service show` is deliberately human presentation and may contain
    // spaces in paths and arguments plus a resolved-link annotation.
    let unit = service::fetch_unit_file(&target, declared, &runner)
        .await
        .map_err(click)?;
    let observed = service::parse_unit_program(&unit)
        .map_err(click)?
        .ok_or_else(|| {
            CmdError::click(format!(
                "{} has no executable program in {}",
                declared.unit_id(),
                unit.path
            ))
        })?;
    let program = observed.as_str();
    let directory = program
        .split("/services/")
        .nth(usize::from(true))
        .and_then(|rest| rest.split('/').next())
        .filter(|segment| !segment.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| declared.name.clone());
    // Two sources, one install. A published artifact is the durable route; a
    // local archive is how a bundle reaches a host before there is an object
    // store the whole fleet can read, and it is checksummed on the far side the
    // same way rather than trusted for arriving.
    if let Some(version) = rollback_to {
        let script = format!(
            "set -euo pipefail\nname={}\nversion={}\n{ROLLBACK_BODY}",
            crate::deploy::shlex_quote(&directory),
            crate::deploy::shlex_quote(version),
        );
        let output = host_channel::run_script(&target, &script, &runner)
            .await
            .map_err(click)?;
        if !output.ok() {
            return Err(CmdError::click(format!(
                "{host}: {}",
                host_channel::last_error_line(&output, "rollback failed")
            )));
        }
        println!("{host}: {name} -> {version} (takes effect on the next restart)");
        return Ok(());
    }
    // The relink is the dangerous half: `current` moves and launchd's next
    // spawn reads a path that may not exist in the tree that just arrived.
    // Checking the archive's member list against the unit's own program path
    // costs one local read and is the difference between a refusal and an
    // outage. On 2026-09-04 the object API unit, whose program is
    // `current/darwin-arm/stado`, was pointed at a published stado archive
    // that holds exactly `bin/stado`; `current` relinked, launchd could not
    // spawn, the job left the system domain, and every `/api/object` read on
    // the fleet failed for eleven minutes.
    if let Some(path) = archive {
        let members = archive_members(path)?;
        refuse_archive_without_program(program, &members).map_err(CmdError::click)?;
    }
    let (installed, already_active) = match (reference, archive) {
        (Some(reference), None) => (
            install_from_artifact(&target, &directory, reference).await?,
            false,
        ),
        (None, Some(path)) => {
            let marker = format!("/services/{directory}/");
            let required = if let Some((_, rest)) = program.split_once(&marker) {
                rest.split_once('/')
                    .map(|(_, tail)| tail.to_string())
                    .ok_or_else(|| {
                        CmdError::click("managed service program has no archive member")
                    })?
            } else {
                let executable = std::path::Path::new(program)
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| CmdError::click("managed service program has no filename"))?;
                format!("darwin-arm/{executable}")
            };
            if !std::path::Path::new(&required)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
            {
                return Err(CmdError::click(
                    "managed service archive member is not relative",
                ));
            }
            install_from_archive(&target, &directory, path, &required, &runner).await?
        }
        (None, None) => {
            return Err(CmdError::click(
                "update needs --from-artifact REF or --from-archive PATH",
            ))
        }
        (Some(_), Some(_)) => {
            return Err(CmdError::click(
                "--from-artifact and --from-archive are exclusive",
            ))
        }
    };
    let followed = if already_active {
        false
    } else {
        // A unit pinned to a version directory never sees an install: `current`
        // moves and the job keeps executing the path it was rendered with.
        follow_current(&target, declared, &directory, &runner).await?
    };
    let image_refresh = if refresh_image {
        let units = service::loaded_units(&target, &runner)
            .await
            .map_err(click)?;
        let before = units
            .iter()
            .find(|unit| unit.label == declared.unit_id())
            .ok_or_else(|| {
                CmdError::click(format!(
                    "{host}: {} is absent from the domain-bound launchd inventory",
                    declared.unit_id()
                ))
            })?;
        if before.pid.is_empty() {
            if before.loaded_domains.len() != 1 {
                return Err(CmdError::click(format!(
                    "{host}: {} has no live pid and {} loaded domains; refusing to guess a lifecycle action",
                    declared.unit_id(),
                    before.loaded_domains.len()
                )));
            }
            let started = service::restart_service(&target, declared, &runner)
                .await
                .map_err(click)?;
            if !started.succeeded("restarted") {
                return Err(CmdError::click(format!(
                    "{host}: {} was confirmed loaded without a live pid and did not start: {}",
                    declared.unit_id(),
                    started.failure()
                )));
            }
        }
        let script = format!(
            "set -euo pipefail\n\"$HOME/.stado/bin/stado\" service refresh-image {} \
             --if-needed --json",
            crate::deploy::shlex_quote(declared.unit_id()),
        );
        let output = host_channel::run_script(&target, &script, &runner)
            .await
            .map_err(click)?;
        if !output.ok() {
            return Err(CmdError::click(format!(
                "{host}: {}",
                host_channel::last_error_line(&output, "the running image did not converge")
            )));
        }
        serde_json::from_str::<Value>(output.stdout.trim()).map_err(|error| {
            CmdError::click(format!(
                "{host}: image refresh returned invalid JSON: {error}; stdout={}",
                output.stdout.trim()
            ))
        })?
    } else {
        Value::Null
    };
    if json {
        print_json(&json!({
            "host": host,
            "service": name,
            "image_refresh": image_refresh,
            "unit_repointed": followed,
            "version": installed.version,
            "sha256": installed.sha256,
            "status": if already_active { "already_installed" } else { "updated" },
            "effective": if refresh_image { "running image verified" } else { "on next restart" },
        }))?;
    } else if refresh_image {
        println!(
            "{host}: {name} -> {} (running image verified)",
            installed.version
        );
    } else {
        println!(
            "{host}: {name} -> {} (takes effect on the next restart)",
            installed.version
        );
    }
    Ok(())
}
